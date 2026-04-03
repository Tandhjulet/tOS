use core::{
    convert::identity,
    ptr::{read_volatile, write_volatile},
};

use alloc::sync::Arc;
use spin::Mutex;
use x86_64::{PhysAddr, VirtAddr};

use crate::{
    allocator::mmio::{PAGE_SIZE, alloc_dma_region},
    filesystem::drivers::StorageDevice,
    pci::{PciDevice, bar::Bar},
    println,
};

/**
 * https://nvmexpress.org/wp-content/uploads/NVMe-NVM-Express-2.0a-2021.07.26-Ratified.pdf
 */
pub mod cfg {
    pub const CAP: u32 = 0x0;
    pub const VS: u32 = 0x08;
    pub const INTMS: u32 = 0x0C;
    pub const INTMC: u32 = 0x10;
    pub const CC: u32 = 0x14;
    pub const CSTS: u32 = 0x1C;
    pub const AQA: u32 = 0x24;
    pub const ASQ: u32 = 0x28;
    pub const ACQ: u32 = 0x30;

    // See figure 138
    pub mod op {
        pub const IDENTIFY: u32 = 0x06;
    }
}

pub struct NVMe {
    device: Arc<Mutex<PciDevice>>,
    cap: ControllerCap,

    identify: Option<IdentifyController>,

    adm_comp_queue: Option<Queue>,
    adm_subm_queue: Option<Queue>,
    adm_buf: (VirtAddr, PhysAddr),
}

impl NVMe {
    pub fn new(device: Arc<Mutex<PciDevice>>) -> Self {
        let mut driver = {
            let binding = device.lock();
            let Some(bar) = PciDevice::get_bar(&binding, 0) else {
                panic!("Could not find BAR0 for NVMe!");
            };

            bar.map_mmio();
            binding.enable_bus_mastering();

            let cap = Self::get_capabilities(&bar);

            Self {
                device: Arc::clone(&device),
                cap,
                identify: None,
                adm_comp_queue: None,
                adm_subm_queue: None,
                adm_buf: alloc_dma_region(PAGE_SIZE),
            }
        };

        driver.init();
        driver
    }

    fn get_capabilities(bar: &Bar) -> ControllerCap {
        let cap = unsafe { bar.read64(cfg::CAP) };
        ControllerCap(cap)
    }

    fn get_configuration(&mut self) -> ControllerConfig {
        let cc = {
            let binding = self.device.lock();
            let Some(bar) = PciDevice::get_bar(&binding, 0) else {
                panic!("Could not find BAR0 for NVMe!");
            };

            unsafe { bar.read32(cfg::CC) }
        };
        ControllerConfig(cc)
    }

    fn init(&mut self) {
        let mut cfg = self.get_configuration();
        cfg.set_enabled(false);
        unsafe { self.write_reg(cfg::CC, cfg.raw()) };

        // wait for controller to disable
        while (unsafe { self.read_reg(cfg::CSTS) } & 0x1) == 1 {}

        let (asq, acq) = self.create_admin_queues();
        self.adm_comp_queue = Some(acq);
        self.adm_subm_queue = Some(asq);

        let mut cfg = self.get_configuration();

        // MPS is defined as page_size = (2 ^ (12 + MPS))
        // since page_size = 4096 => MPS = 0
        let mps = 0;

        let mut css = 0b000u8;
        if self.cap.css_none() {
            css = 0b111;
        } else if self.cap.css_some() {
            css = 0b110;
        }

        cfg.set_css(css)
            .set_ams(AmsType::RoundRobin)
            .set_mps(mps)
            .set_enabled(false)
            .set_iocqes(4) // Comp entry size: 2^4 = 16 bytes
            .set_iosqes(6); // Subm entry size: 2^6 = 64 bytes

        unsafe { self.write_reg(cfg::CC, cfg.raw()) };

        cfg.set_enabled(true);
        unsafe { self.write_reg(cfg::CC, cfg.raw()) };

        // wait for controller to enable
        while (unsafe { self.read_reg(cfg::CSTS) } & 0x1) == 0 {}

        println!("retrieving identity data structure");

        let identify = self.identify_controller();
        self.identify = Some(identify);

        println!("VID: {}", identify.vid);
    }

    fn identify_controller(&mut self) -> IdentifyController {
        let mut identify = SQEntry::default();
        identify.cdw0 = cfg::op::IDENTIFY | (1 << 16);
        identify.prp1 = self.adm_buf.1.as_u64();
        identify.cdw10 = 0x1;

        self.submit_admin_command(identify);
        let identify = unsafe { *(self.adm_buf.0.as_ptr::<IdentifyController>()) };
        identify
    }

    pub unsafe fn write_reg(&self, offset: u32, val: u32) {
        let binding = self.device.lock();
        let Some(bar) = binding.get_bar(0) else {
            panic!("Failed to access BAR0!");
        };

        unsafe {
            bar.write32(offset, val);
        };
    }

    pub unsafe fn read_reg(&self, offset: u32) -> u32 {
        let binding = self.device.lock();
        let Some(bar) = binding.get_bar(0) else {
            panic!("Failed to access BAR0!");
        };

        unsafe { bar.read32(offset) }
    }

    fn create_admin_queues(&self) -> (Queue, Queue) {
        let binding = self.device.lock();
        let Some(bar) = PciDevice::get_bar(&binding, 0) else {
            panic!("Could not find BAR0 for NVMe!");
        };

        let mut asq = Queue::default();
        let mut acq = Queue::default();

        let (asq_virt, asq_phys) = alloc_dma_region(PAGE_SIZE);
        let (acq_virt, acq_phys) = alloc_dma_region(PAGE_SIZE);

        asq.queue_phys = asq_phys.as_u64();
        acq.queue_phys = acq_phys.as_u64();

        asq.queue_virt = asq_virt.as_u64();
        acq.queue_virt = acq_virt.as_u64();

        asq.size = 63;
        acq.size = 63;

        asq.phase = 1;
        acq.phase = 1;

        let aqa = ((acq.size as u32) << 16) | (asq.size as u32);
        unsafe { bar.write32(cfg::AQA, aqa) };

        unsafe {
            bar.write64(cfg::ASQ, asq.queue_phys);
            bar.write64(cfg::ACQ, acq.queue_phys);
        }

        (asq, acq)
    }

    fn submit_admin_command(&mut self, cmd: SQEntry) -> CQEntry {
        let sq = self.adm_subm_queue.as_mut().unwrap();

        let slot = sq.queue_virt + (sq.tail as u64 * size_of::<SQEntry>() as u64);
        unsafe {
            write_volatile(slot as *mut SQEntry, cmd);
        };

        sq.tail = (sq.tail + 1) % (sq.size as u16 + 1);
        let tail = sq.tail;
        let doorbell = self.sq_doorbell(0);
        unsafe {
            self.write_reg(doorbell, tail as u32);
        };

        self.poll_admin_completion()
    }

    fn poll_admin_completion(&mut self) -> CQEntry {
        loop {
            println!("polling...");
            let (slot, phase) = {
                let cq = self.adm_comp_queue.as_mut().unwrap();
                let slot = cq.queue_virt + (cq.head as u64 * size_of::<CQEntry>() as u64);
                (slot, cq.phase)
            };

            let entry = unsafe { read_volatile(slot as *const CQEntry) };

            if entry.status & 0x1 == phase as u16 {
                let doorbell = self.cq_doorbell(0);
                let new_head = {
                    let cq = self.adm_comp_queue.as_mut().unwrap();
                    cq.head += 1;
                    if cq.head > cq.size as u16 {
                        cq.head = 0;
                        cq.phase ^= 1; // flip phase on wraparound
                    }

                    cq.head
                };

                unsafe { self.write_reg(doorbell, new_head as u32) };

                if (entry.status >> 1) != 0 {
                    panic!("NVMe admin command failed: status={:#x}", entry.status >> 1);
                }

                return entry;
            }
        }
    }

    fn sq_doorbell(&self, queue_id: u16) -> u32 {
        0x1000 + (2 * queue_id as u32) * (4 << self.cap.dstrd() as u32)
    }

    fn cq_doorbell(&self, queue_id: u16) -> u32 {
        0x1000 + (2 * queue_id as u32 + 1) * (4 << self.cap.dstrd() as u32)
    }
}

pub struct ControllerConfig(u32);

impl ControllerConfig {
    pub fn raw(&self) -> u32 {
        self.0
    }

    pub fn set_raw(&mut self, raw: u32) -> &mut Self {
        self.0 = raw;
        self
    }

    pub fn set_css_from_cap(&mut self, cap: &ControllerCap) -> &mut Self {
        let mut css = 0b000u8;
        if cap.css_none() {
            css = 0b111;
        } else if cap.css_some() {
            css = 0b110;
        }
        self.set_css(css);
        self
    }

    pub fn set_css(&mut self, css: u8) -> &mut Self {
        self.0 = self.0 & !(0x7 << 4) | ((css as u32 & 0x7) << 4);
        self
    }

    pub fn set_enabled(&mut self, en: bool) -> &mut Self {
        self.0 = (self.0 & !0x1) | (en as u32);
        self
    }

    pub fn set_iosqes(&mut self, iosqes: u32) -> &mut Self {
        self.0 = (self.0 & !(0x7 << 16)) | ((iosqes & 0x7) << 16);
        self
    }

    pub fn set_iocqes(&mut self, iocqes: u32) -> &mut Self {
        self.0 = (self.0 & !(0x7 << 20)) | ((iocqes & 0x7) << 20);
        self
    }

    pub fn set_ams(&mut self, ams: AmsType) -> &mut Self {
        self.0 = (self.0 & !(0b11 << 11)) | ((ams as u32) << 11);
        self
    }

    pub fn set_mps(&mut self, mps: u32) -> &mut Self {
        self.0 = (self.0 & !(0x7 << 7)) | ((mps & 0x7) << 7);
        self
    }
}

#[repr(u8)]
pub enum AmsType {
    RoundRobin = 0b000,
    WeightedRoundRobin = 0b001,
    Vendor = 0b111,
}

pub struct ControllerCap(u64);

impl ControllerCap {
    pub fn dstrd(&self) -> u8 {
        ((self.0 >> 32) & 0xF) as u8
    }

    pub fn mqes(&self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    pub fn to(&self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }

    pub fn mpsmin(&self) -> u8 {
        ((self.0 >> 48) & 0xF) as u8
    }

    /**
     * Command Sets Supported (CSS)
     */
    pub fn css(&self) -> u8 {
        ((self.0 >> 37) & 0xFF) as u8
    }

    pub fn css_nvm(&self) -> bool {
        self.css() & 0x1 == 1
    }

    pub fn css_none(&self) -> bool {
        (self.css() >> 7) & 0x1 == 1
    }

    pub fn css_some(&self) -> bool {
        (self.css() >> 6) & 0x1 == 1
    }
}

#[derive(Default)]
struct Queue {
    queue_phys: u64,
    queue_virt: u64,
    size: u64,

    tail: u16,
    head: u16,
    phase: u8,
}

#[derive(Default)]
#[repr(C)]
pub struct SQEntry {
    pub cdw0: u32,
    pub nsid: u32,
    pub reserved: u64,
    pub mptr: u64,
    pub prp1: u64,
    pub prp2: u64,
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

#[repr(C)]
pub struct CQEntry {
    pub dw0: u32,
    pub dw1: u32,
    pub sq_head: u16,
    pub sq_id: u16,
    pub cid: u16,
    pub status: u16,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyController {
    pub vid: u16,
    pub ssvid: u16,
    pub sn: [u8; 20],
    pub mn: [u8; 40],
    pub fr: [u8; 8],
    pub rab: u8,
    // see Figure 275 for the rest of the fields... there's a lot
}

impl StorageDevice for NVMe {
    fn read() {
        todo!()
    }

    fn write() {
        todo!()
    }
}
