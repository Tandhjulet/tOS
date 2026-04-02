use alloc::sync::Arc;
use spin::Mutex;

use crate::{
    allocator::mmio::{PAGE_SIZE, alloc_dma_region},
    filesystem::drivers::StorageDevice,
    pci::{
        PciDevice,
        bar::{Bar, BarKind},
    },
    println,
};

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
}

pub struct NVMe {
    device: Arc<Mutex<PciDevice>>,
    cap: ControllerCap,

    adm_comp_queue: Option<Queue>,
    adm_subm_queue: Option<Queue>,
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
                adm_comp_queue: None,
                adm_subm_queue: None,
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
        while (unsafe { self.read_reg(cfg::CSTS) } & 0x1) != 0 {}

        let (asq, acq) = self.create_admin_queues();
        self.adm_comp_queue = Some(acq);
        self.adm_subm_queue = Some(asq);

        let mut cfg = self.get_configuration();

        // MPS is defined as page_size = (2 ^ (12 + MPS))
        // since page_size = 4096 => MPS = 0
        let mps = 0;

        cfg.set_css_from_cap(&self.cap)
            .set_ams(AmsType::RoundRobin)
            .set_mps(mps)
            .set_enabled(true);

        unsafe {
            self.write_reg(cfg::CC, cfg.raw());
        };

        println!("init!");
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

        let (_, asq_phys) = alloc_dma_region(PAGE_SIZE);
        let (_, acq_phys) = alloc_dma_region(PAGE_SIZE);

        asq.queue_addr = asq_phys.as_u64();
        acq.queue_addr = acq_phys.as_u64();
        asq.size = 63;
        acq.size = 63;

        let aqa = ((acq.size as u32) << 16) | (asq.size as u32);
        unsafe { bar.write32(cfg::AQA, aqa) };

        unsafe {
            bar.write64(cfg::ASQ, asq.queue_addr);
            bar.write64(cfg::ACQ, acq.queue_addr);
        }

        (asq, acq)
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
    queue_addr: u64,
    size: u64,
}

#[repr(C)]
struct SQueueEntry {
    pub cdw0: u32,
    pub nsid: u32,
    pub reserved: u64,
    pub mptr: u64,
    pub dptr1: u64,
    pub dptr2: u64,
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

impl StorageDevice for NVMe {
    fn read() {
        todo!()
    }

    fn write() {
        todo!()
    }
}
