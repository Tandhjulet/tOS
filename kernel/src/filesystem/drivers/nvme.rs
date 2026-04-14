use core::{
    cmp::min,
    marker::PhantomData,
    ptr::{read_volatile, write_volatile},
};

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use log::error;
use spin::Mutex;
use x86_64::align_up;

use crate::{
    allocator::mmio::{MappedRegion, PAGE_SIZE, alloc_dma_region, map_mmio},
    filesystem::drivers::StorageDevice,
    io::pci::{PciDevice, bar::Bar},
    println,
    sys::interrupts::{
        self, INTERRUPT_CONTROLLER, InterruptControllerType, InterruptMode, IrqResult,
    },
};

///
/// NVMe Documentation:
/// - Base specification: https://nvmexpress.org/wp-content/uploads/NVMe-NVM-Express-2.0a-2021.07.26-Ratified.pdf
/// - NVMe over PCIe specification: https://nvmexpress.org/wp-content/uploads/NVM-Express-NVMe-over-PCIe-Transport-Specification-Revision-1.3-2025.08.01-Ratified.pdf
/// - NVM Host Controller Interface (has good overview over commands): https://www.nvmexpress.org/wp-content/uploads/NVM-Express-1_1a.pdf
///
pub mod cfg {
    pub const IO_QUEUES: u16 = 2;

    pub const CAP: u32 = 0x0;
    pub const VS: u32 = 0x08;
    pub const INTMS: u32 = 0x0C;
    pub const INTMC: u32 = 0x10;
    pub const CC: u32 = 0x14;
    pub const CSTS: u32 = 0x1C;
    pub const AQA: u32 = 0x24;
    pub const ASQ: u32 = 0x28;
    pub const ACQ: u32 = 0x30;

    // See figure 138 for at list of operations
    pub mod op {
        pub const IDENTIFY: u32 = 0x06;
        pub const SET_FEATURES: u32 = 0x09;
        pub const GET_FEATURES: u32 = 0x0A;

        // For a list of Identify CNS values and reference sections, view figure 273
        pub mod identify {
            pub const CNS_NAMESPACE: u32 = 0x0;
            pub const CNS_CONTROLLER: u32 = 0x1;
            pub const CNS_SPECIFIC_NS: u32 = 0x5;
            pub const CNS_SPECIFIC_CTRLR: u32 = 0x6;
            pub const CNS_ACTIVE_NS_CMD_SET: u32 = 0x7;
            pub const CNS_NAMESPACE_INDEPENDENT: u32 = 0x8;
            pub const CNS_CMD_SET: u32 = 0x1C;
        }

        pub mod features {
            pub const FID_NUM_QUEUES: u32 = 0x07;
            pub const FID_SET_PROFILE: u32 = 0x19;
        }
    }
}

pub struct NvmeNamespace {
    pub nsid: u32,
    pub csi: u8,

    // CNS 0x00 - only NVM-based command sets (NVM and Zoned NS)
    // Contains LBA formats, cap, metadata cap
    pub nvm_base: Option<IdentifyNamespaceNvm>,

    // CNS 0x08 - command-set-indepedent fields
    pub independent: IdentifyNamespaceIndependent,
}

pub struct NvmeController {
    device: Arc<Mutex<PciDevice>>,
    cap: ControllerCap,

    identify_ctlr: Option<IdentifyController>,

    adm_queue: Option<QueuePair>,
    adm_buf: MappedRegion,

    io_queue: Vec<QueuePair>,

    namespaces: Vec<NvmeNamespace>,
}

impl NvmeController {
    pub fn new(device: Arc<Mutex<PciDevice>>) -> Arc<Mutex<NvmeController>> {
        let driver = {
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
                identify_ctlr: None,
                adm_queue: None,
                io_queue: Vec::new(),
                adm_buf: alloc_dma_region(PAGE_SIZE),
                namespaces: Vec::new(),
            }
        };

        let driver = Arc::new(Mutex::new(driver));
        NvmeController::init(&driver);
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

    fn init(this: &Arc<Mutex<Self>>) {
        let mut controller = this.lock();

        let mut cfg = controller.get_configuration();
        cfg.set_enabled(false);
        unsafe { controller.write_reg(cfg::CC, cfg.raw()) };

        // wait for controller to disable
        while (unsafe { controller.read_reg(cfg::CSTS) } & 0x1) == 1 {}

        let admin_queues = controller.create_admin_queues();
        controller.adm_queue = Some(admin_queues);

        let mut cfg = controller.get_configuration();

        // MPS is defined as page_size = (2 ^ (12 + MPS))
        // since page_size = 4096 => MPS = 0
        let mps = 0;

        let mut css = 0b000u8;
        if controller.cap.css_none() {
            css = 0b111;
        } else if controller.cap.css_some() {
            css = 0b110;
        }

        cfg.set_css(css)
            .set_ams(AmsType::RoundRobin)
            .set_mps(mps)
            .set_enabled(false)
            .set_iocqes(4) // Comp entry size: 2^4 = 16 bytes
            .set_iosqes(6); // Subm entry size: 2^6 = 64 bytes

        unsafe { controller.write_reg(cfg::CC, cfg.raw()) };

        cfg.set_enabled(true);
        unsafe { controller.write_reg(cfg::CC, cfg.raw()) };

        // wait for controller to enable
        while (unsafe { controller.read_reg(cfg::CSTS) } & 0x1) == 0 {}

        let identify_ctrlr = controller
            .identify_read::<IdentifyController>(cfg::op::identify::CNS_CONTROLLER, |_| {});
        controller.identify_ctlr = Some(identify_ctrlr);

        if cfg.css() == 0 {
            // TODO
        }
        if controller.cap.css_some() {
            let cmd_set = controller
                .identify_read::<IdentifyCommandSet>(cfg::op::identify::CNS_CMD_SET, |_| {});
            let selected_cmd_idx = cmd_set.first_valid().unwrap();

            // Refer to section 5.27.1.21 for documentation regarding
            // I/O Command Set Profile (FID: 0x19)
            controller.set_features(cfg::op::features::FID_SET_PROFILE, |features| {
                features.cdw11 = selected_cmd_idx as u32;
            });

            for csi in cmd_set.csi_iter(selected_cmd_idx) {
                let nsids = controller.identify_read::<IdentifyNamespaceList>(
                    cfg::op::identify::CNS_ACTIVE_NS_CMD_SET,
                    |identify| {
                        identify.nsid = 0;

                        // See figure 271
                        identify.cdw11 = (csi as u32) << 24;
                    },
                );

                for &nsid in nsids.valid() {
                    let nvm_base = if Self::is_csi_nvm_based(csi) {
                        let ns_nvm = controller.identify_read::<IdentifyNamespaceNvm>(
                            cfg::op::identify::CNS_NAMESPACE,
                            |cmd| {
                                cmd.nsid = nsid;
                            },
                        );

                        Some(ns_nvm)
                    } else {
                        None
                    };

                    // TODO: store this; it's CSI-dependent and contains specific meta
                    controller.identify(cfg::op::identify::CNS_SPECIFIC_NS, |cmd| {
                        cmd.nsid = nsid;
                        cmd.cdw11 = (csi as u32) << 24;
                    });

                    controller.identify(cfg::op::identify::CNS_SPECIFIC_CTRLR, |cmd| {
                        cmd.cdw11 = (csi as u32) << 24;
                    });

                    let independent = controller.identify_read::<IdentifyNamespaceIndependent>(
                        cfg::op::identify::CNS_NAMESPACE_INDEPENDENT,
                        |cmd| {
                            cmd.nsid = nsid;
                        },
                    );

                    controller.namespaces.push(NvmeNamespace {
                        nsid,
                        csi,
                        nvm_base,
                        independent,
                    });
                }
            }
        }

        let queue_cnt = controller.init_queue_cnt();

        match controller.setup_pci_interrupt_mode() {
            InterruptMode::MsiX => {
                let lock = controller.device.lock();
                let mut map = lock
                    .get_msix_tables()
                    .expect("MSI-X tables should be present as MSI-X is enabled");

                let cpu_id = match &*INTERRUPT_CONTROLLER.get() {
                    InterruptControllerType::Apic(apic_info) => apic_info.lapic.id(),
                    _ => panic!("Using MSI-X, the interrupt controller should always be APIC!"),
                };

                let weak = Arc::downgrade(&this);
                let admin_vector = interrupts::allocate_interrupt(Box::new(move || {
                    if let Some(ctrlr) = weak.upgrade() {
                        ctrlr.lock().nvme_int_handler()
                    } else {
                        IrqResult::EoiNeeded
                    }
                }))
                .expect("should be available interrupt vectors");

                map[0].init(cpu_id, admin_vector as u32);
            }
            InterruptMode::Msi => todo!(),
            InterruptMode::Legacy => todo!(),
        }
    }

    // pub fn create_io_subm_queue(
    //     &self,
    //     max_entries: usize,
    //     id: u32,
    //     comp_id: u32,
    // ) -> Queue<Submission> {
    //     let size = max_entries * size_of::<SQEntry>();
    //     let page_count = align_up(size as u64, PAGE_SIZE) / PAGE_SIZE;

    //     let pages = alloc_dma_region(size as u64);

    //     let entry = SQEntry::default();
    //     entry.prp1 = pages.phys().as_u64();

    //     entry.cdw10 = (id & 0xfffff) | ((max_entries as u32 - 1) << 16);

    //     const PHYS_CONTIG: u32 = 1;
    //     entry.cdw11 = PHYS_CONTIG | (comp_id << 16);

    //     let cq = self.submit_admin_command(entry);
    // }

    // pub fn create_io_comp_queue(&self) -> Queue<Completion> {}

    // pub fn create_io_queue(&self) -> QueuePair {
    //     let subm = self.create_io_subm_queue();
    //     let comp = self.create_io_comp_queue();

    //     QueuePair { subm: (), comp: () }
    // }

    fn init_queue_cnt(&mut self) -> u16 {
        let io_queue_count_raw = self.set_features(cfg::op::features::FID_NUM_QUEUES, |cmd| {
            cmd.cdw11 = ((cfg::IO_QUEUES as u32) << 16) | cfg::IO_QUEUES as u32;
        });

        let io_comp_queues = (io_queue_count_raw.dw0 >> 16) as u16;
        let io_subm_queues = io_queue_count_raw.dw0 as u16;

        min(io_comp_queues, io_subm_queues)
    }

    pub fn nvme_int_handler(&self) -> IrqResult {
        println!("IRQ!");
        IrqResult::EoiNeeded
    }

    pub fn setup_pci_interrupt_mode(&self) -> InterruptMode {
        let mut interrupt_mode: Option<InterruptMode> = None;
        let supported_interrupts = self.device.lock().interrupt_support();
        if supported_interrupts.msix {
            match self.device.lock().enable_msix() {
                Ok(_) => interrupt_mode = Some(InterruptMode::MsiX),
                Err(msg) => error!("NVMe MSI-X: {}", msg),
            }
        }

        if supported_interrupts.msi && interrupt_mode.is_none() {
            todo!()
        }

        interrupt_mode.unwrap_or_else(|| {
            panic!("NVMe: no suitable interrupt mode could be enabled");
        })
    }

    fn is_csi_nvm_based(csi: u8) -> bool {
        matches!(csi, 0x00 | 0x03)
    }

    fn identify_read<T: Copy>(&mut self, cns: u32, cmd: impl FnOnce(&mut SQEntry)) -> T {
        self.identify(cns, cmd);
        let identify = unsafe { *(self.adm_buf.as_ptr::<T>()) };
        identify
    }

    fn identify(&mut self, cns: u32, cmd: impl FnOnce(&mut SQEntry)) -> CQEntry {
        let mut identify = SQEntry::default();
        identify.cdw0 = cfg::op::IDENTIFY | (1 << 16);
        identify.prp1 = self.adm_buf.phys().as_u64();
        identify.cdw10 = cns;

        cmd(&mut identify);

        self.submit_admin_command(identify)
    }

    fn set_features(&mut self, fid: u32, cmd: impl FnOnce(&mut SQEntry)) -> CQEntry {
        let mut features = SQEntry::default();
        features.cdw0 = cfg::op::SET_FEATURES | (1 << 16);
        features.prp1 = self.adm_buf.phys().as_u64();
        features.cdw10 = fid;

        cmd(&mut features);

        self.submit_admin_command(features)
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

    fn create_admin_queues(&self) -> QueuePair {
        let binding = self.device.lock();
        let Some(bar) = PciDevice::get_bar(&binding, 0) else {
            panic!("Could not find BAR0 for NVMe!");
        };

        let mut asq = Queue::default();
        let mut acq = Queue::default();

        asq.region = Some(alloc_dma_region(PAGE_SIZE));
        acq.region = Some(alloc_dma_region(PAGE_SIZE));

        asq.state.size = 63;
        acq.state.size = 63;

        let aqa = ((acq.state.size as u32) << 16) | (asq.state.size as u32);
        unsafe { bar.write32(cfg::AQA, aqa) };

        unsafe {
            bar.write64(cfg::ASQ, asq.phys().unwrap());
            bar.write64(cfg::ACQ, acq.phys().unwrap());
        }

        QueuePair {
            subm: asq,
            comp: acq,
        }
    }

    fn submit_admin_command(&mut self, cmd: SQEntry) -> CQEntry {
        let sq = &mut self.adm_queue.as_mut().unwrap().subm;

        let slot = sq.virt().unwrap() + (sq.state.tail as u64 * size_of::<SQEntry>() as u64);
        unsafe {
            write_volatile(slot as *mut SQEntry, cmd);
        };

        sq.state.tail = (sq.state.tail + 1) % (sq.state.size as u16 + 1);
        let tail = sq.state.tail;
        let doorbell = self.sq_doorbell(0);
        unsafe {
            self.write_reg(doorbell, tail as u32);
        };

        self.poll_admin_completion()
    }

    fn poll_admin_completion(&mut self) -> CQEntry {
        loop {
            let (slot, phase) = {
                let cq = &self.adm_queue.as_mut().unwrap().comp;
                let slot =
                    cq.virt().unwrap() + (cq.state.head as u64 * size_of::<CQEntry>() as u64);
                (slot, cq.state.phase)
            };

            // info!("phase: {}", phase);

            let entry = unsafe { read_volatile(slot as *const CQEntry) };

            if entry.status.phase_tag() == phase {
                let doorbell = self.cq_doorbell(0);
                let new_head = {
                    let cq = &mut self.adm_queue.as_mut().unwrap().comp;
                    cq.state.head += 1;
                    if cq.state.head > cq.state.size as u16 {
                        cq.state.head = 0;
                        cq.state.phase = !cq.state.phase; // flip phase on wraparound
                    }

                    cq.state.head
                };

                unsafe { self.write_reg(doorbell, new_head as u32) };

                if !entry.status.is_success() {
                    panic!("NVMe admin command failed: status={:?}", entry.status);
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

    pub fn css(&mut self) -> u8 {
        ((self.0 >> 4) & 0x7) as u8
    }

    pub fn set_enabled(&mut self, en: bool) -> &mut Self {
        self.0 = (self.0 & !0x1) | (en as u32);
        self
    }

    pub fn set_iosqes(&mut self, iosqes: u32) -> &mut Self {
        self.0 = (self.0 & !(0xF << 16)) | ((iosqes & 0xF) << 16);
        self
    }

    pub fn set_iocqes(&mut self, iocqes: u32) -> &mut Self {
        self.0 = (self.0 & !(0xF << 20)) | ((iocqes & 0xF) << 20);
        self
    }

    pub fn set_ams(&mut self, ams: AmsType) -> &mut Self {
        self.0 = (self.0 & !(0b111 << 11)) | ((ams as u32 & 0b111) << 11);
        self
    }

    pub fn set_mps(&mut self, mps: u32) -> &mut Self {
        self.0 = (self.0 & !(0xF << 7)) | ((mps & 0xF) << 7);
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

    ///
    /// Command Sets Supported (CSS)
    ///
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

trait QueueKind {}

#[derive(Default)]
struct Submission;
#[derive(Default)]
struct Completion;

impl QueueKind for Submission {}
impl QueueKind for Completion {}

struct RingQueueState {
    size: u64,

    tail: u16,
    head: u16,
    phase: bool,
}

impl Default for RingQueueState {
    fn default() -> Self {
        Self {
            size: Default::default(),
            tail: Default::default(),
            head: Default::default(),
            phase: true,
        }
    }
}

struct QueuePair {
    subm: Queue<Submission>,
    comp: Queue<Completion>,
}

#[derive(Default)]
struct Queue<K: QueueKind> {
    region: Option<MappedRegion>,
    state: RingQueueState,
    _phantom: PhantomData<K>,
}

impl<K: QueueKind> Queue<K> {
    pub fn phys(&self) -> Option<u64> {
        self.region.as_ref().map(|r| r.phys().as_u64())
    }

    pub fn virt(&self) -> Option<u64> {
        self.region.as_ref().map(|r| r.virt().as_u64())
    }
}

impl Queue<Submission> {}

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
    pub status: Status,
}

#[derive(Debug)]
#[repr(transparent)]
pub struct Status(u16);

impl Status {
    pub fn phase_tag(&self) -> bool {
        self.0 & 1 != 0
    }

    pub fn code(&self) -> u8 {
        ((self.0 >> 1) & 0xFF) as u8
    }

    pub fn code_type(&self) -> u8 {
        ((self.0 >> 9) & 0x7) as u8
    }

    pub fn more(&self) -> bool {
        self.0 & (1 << 14) != 0
    }

    pub fn do_not_retry(&self) -> bool {
        self.0 & (1 << 15) > 0
    }

    pub fn is_success(&self) -> bool {
        self.code_type() == 0 && self.code() == 0
    }
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

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyCommandSet {
    pub iocsc: [u64; 512],
}

impl IdentifyCommandSet {
    pub fn is_nvm_supported(cmd_set: u64) -> bool {
        cmd_set & 0x1 == 1
    }

    pub fn is_kv_supported(cmd_set: u64) -> bool {
        cmd_set & 0x2 == 1
    }

    pub fn is_zns_supported(cmd_set: u64) -> bool {
        cmd_set & 0x4 == 1
    }

    pub fn first_valid(&self) -> Option<usize> {
        self.iocsc.iter().position(|&e| Self::is_nvm_supported(e))
    }

    pub fn csi_iter(&self, idx: usize) -> impl Iterator<Item = u8> {
        let entry = self.iocsc[idx];
        (0u8..3).filter(move |&bit| entry & (1 << bit) == 1)
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyNamespaceList {
    pub namespaces: [u32; 1024],
}

impl IdentifyNamespaceList {
    pub fn valid(&self) -> impl Iterator<Item = &u32> {
        self.namespaces.iter().take_while(|&&n| n != 0)
    }
}

///
/// Refer to https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-Revision-1.1-2024.08.05-Ratified.pdf
/// figure 114 for documentation regarding the implementation
///
#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyNamespaceNvm {
    pub nsze: u64,             // size
    pub ncap: u64,             // capacity
    pub nuse: u64,             // utilization
    pub nsfeat: u8,            // features
    pub nlbaf: u8,             // number of LBA formats
    pub flbas: u8,             // formatted LBA size
    pub _reserved: [u8; 73],   // fields 0x1B - 0x63 are not yet implemented
    pub lbaf: [LbaFormat; 64], // lba format support
    pub _pad: [u8; 3740],
}

impl IdentifyNamespaceNvm {
    pub fn active_lbaf_idx(&self) -> usize {
        let low = (self.flbas & 0xF) as usize;
        let high = (self.flbas >> 5 & 0x3) as usize;
        (high << 4) | low
    }

    pub fn active_lbaf(&self) -> LbaFormat {
        self.lbaf[self.active_lbaf_idx()]
    }

    pub fn block_size(&self) -> u64 {
        1 << self.active_lbaf().lbads
    }

    pub fn size_bytes(&self) -> u64 {
        self.nsze * self.block_size()
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct LbaFormat {
    pub ms: u16,   // Metadata Size per LBA
    pub lbads: u8, // LBA Data Size (reported as 2^self.lbads)
    pub rp: u8,    // Relative Performance
}

// See Figure 280 in the base specification
#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyNamespaceIndependent {
    pub nsfeat: u8,    // namespace features
    pub nmic: u8,      // multi-path I/O and sharing capabilities
    pub rescap: u8,    // reservation capabilities
    pub fpi: u8,       // format progress indicator
    pub anagrpid: u32, // ANA group identifier
    pub nsattr: u8,    // namespace attributes
    pub _reserved: u8,
    pub nvmsetid: u16, // NVM set identifier
    pub endgid: u16,   // endurance group identifier
    pub nstat: u8,     // namespace status
    pub _reserved2: [u8; 4081],
}

impl StorageDevice for NvmeController {
    fn read() {
        todo!()
    }

    fn write() {
        todo!()
    }
}
