use core::{
    cmp::min,
    ptr::{read_volatile, write_volatile},
    sync::atomic::AtomicUsize,
};

use alloc::{boxed::Box, format, sync::Arc, vec::Vec};
use log::error;
use spin::Mutex;

use crate::{
    allocator::mmio::{MappedRegion, PAGE_SIZE, alloc_dma_region},
    filesystem::block::{
        DeviceId, REGISTRY,
        nvme::{
            namespace::{
                IdentifyNamespaceIndependent, IdentifyNamespaceList, IdentifyNamespaceNvm,
                NvmeNamespace,
            },
            queue::{CQEntry, Completion, Queue, QueuePair, SQEntry, Submission},
        },
    },
    io::pci::{PciDevice, bar::Bar},
    println,
    sys::interrupts::{
        self, INTERRUPT_CONTROLLER, InterruptControllerType, InterruptMode, IrqResult,
    },
};

pub mod namespace;
pub mod queue;
pub mod spec;

static NVME_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct NvmeController {
    device: Arc<Mutex<PciDevice>>,
    cap: ControllerCap,

    identify_ctlr: Option<IdentifyController>,

    adm_queue: Option<QueuePair>,
    adm_buf: MappedRegion,

    queues: Vec<QueuePair>,
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
                queues: Vec::new(),
                adm_buf: alloc_dma_region(PAGE_SIZE),
            }
        };

        let driver = Arc::new(Mutex::new(driver));
        NvmeController::init(&driver);
        driver
    }

    fn get_capabilities(bar: &Bar) -> ControllerCap {
        let cap = unsafe { bar.read64(spec::CAP) };
        ControllerCap(cap)
    }

    fn get_configuration(&mut self) -> ControllerConfig {
        let cc = {
            let binding = self.device.lock();
            let Some(bar) = PciDevice::get_bar(&binding, 0) else {
                panic!("Could not find BAR0 for NVMe!");
            };

            unsafe { bar.read32(spec::CC) }
        };
        ControllerConfig(cc)
    }

    fn init(this: &Arc<Mutex<Self>>) {
        let namespaces: Vec<NvmeNamespace> = {
            let mut controller = this.lock();
            controller.reset_and_disable();
            let mut cfg = controller.configure();
            controller.enable(&mut cfg);
            controller.run_identify_seq(&mut cfg);

            let namespaces = controller.enumerate_namespaces();
            namespaces
        };

        {
            let mut controller = this.lock();
            let queue_cnt = controller.init_queue_cnt() as u32;

            controller.setup_interrupts(this, queue_cnt);
            controller.create_io_queues(queue_cnt);
        };

        let mut registry = REGISTRY.lock();
        let nvme_id = NVME_COUNTER.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        for ns in namespaces {
            if let Some(base) = ns.nvm_base {
                let id = DeviceId(format!("nvme{}n{}", nvme_id, ns.nsid));

                let block_size = base.block_size();
                let block_count = base.block_count();

                registry.register(id, Arc::new(Mutex::new(ns)), block_size, block_count)
            }
        }
    }

    fn create_io_queues(&mut self, queue_cnt: u32) {
        let slots_needed = queue_cnt as usize - self.queues.len();
        self.queues.reserve_exact(slots_needed);

        const ENTRY_COUNT: usize = PAGE_SIZE as usize / size_of::<SQEntry>();
        for i in 0..queue_cnt as u16 {
            let pair = self.create_queue_pair(ENTRY_COUNT, i + 1);
            self.queues.push(pair);
        }
    }

    fn run_identify_seq(&mut self, cfg: &mut ControllerConfig) {
        let identify_ctrlr =
            self.identify_read::<IdentifyController>(spec::op::identify::CNS_CONTROLLER, |_| {});
        self.identify_ctlr = Some(identify_ctrlr);

        if cfg.css() == 0 {
            // TODO
        }
    }

    fn enumerate_namespaces(&mut self) -> Vec<NvmeNamespace> {
        let mut namespaces = Vec::new();
        let cmd_set =
            self.identify_read::<IdentifyCommandSet>(spec::op::identify::CNS_CMD_SET, |_| {});
        let selected_cmd_idx = cmd_set.first_valid().unwrap();

        // Refer to section 5.27.1.21 for documentation regarding
        // I/O Command Set Profile (FID: 0x19)
        self.set_features(spec::op::features::FID_SET_PROFILE, |features| {
            features.cdw11 = selected_cmd_idx as u32;
        });

        for csi in cmd_set.csi_iter(selected_cmd_idx) {
            let nsids = self.identify_read::<IdentifyNamespaceList>(
                spec::op::identify::CNS_ACTIVE_NS_CMD_SET,
                |identify| {
                    identify.nsid = 0;

                    // See figure 271
                    identify.cdw11 = (csi as u32) << 24;
                },
            );

            for &nsid in nsids.valid() {
                let namespace = self.build_namespace(nsid, csi);
                namespaces.push(namespace);
            }
        }

        namespaces
    }

    fn build_namespace(&mut self, nsid: u32, csi: u8) -> NvmeNamespace {
        let nvm_base = if Self::is_csi_nvm_based(csi) {
            let ns_nvm = self.identify_read::<IdentifyNamespaceNvm>(
                spec::op::identify::CNS_NAMESPACE,
                |cmd| {
                    cmd.nsid = nsid;
                },
            );

            Some(ns_nvm)
        } else {
            None
        };

        // TODO: store CSI-specific meta
        self.identify(spec::op::identify::CNS_SPECIFIC_NS, |cmd| {
            cmd.nsid = nsid;
            cmd.cdw11 = (csi as u32) << 24;
        });

        self.identify(spec::op::identify::CNS_SPECIFIC_CTRLR, |cmd| {
            cmd.cdw11 = (csi as u32) << 24;
        });

        let independent = self.identify_read::<IdentifyNamespaceIndependent>(
            spec::op::identify::CNS_NAMESPACE_INDEPENDENT,
            |cmd| {
                cmd.nsid = nsid;
            },
        );

        NvmeNamespace {
            nsid,
            csi,
            nvm_base,
            independent,
        }
    }

    fn reset_and_disable(&mut self) {
        let mut cfg = self.get_configuration();
        cfg.set_enabled(false);
        unsafe { self.write_reg(spec::CC, cfg.raw()) };

        // wait for controller to disable
        while (unsafe { self.read_reg(spec::CSTS) } & 0x1) == 1 {}

        let admin_queues = self.create_admin_queues();
        self.adm_queue = Some(admin_queues);
    }

    fn configure(&mut self) -> ControllerConfig {
        // MPS: page_size = (2 ^ (12 + MPS)), so 4096-byte pages => MPS = 0
        let mps = 0;
        let css = self.select_command_set_selector();

        let mut cfg = self.get_configuration();

        cfg.set_css(css)
            .set_ams(AmsType::RoundRobin)
            .set_mps(mps)
            .set_enabled(false)
            .set_iocqes(4) // Comp entry size: 2^4 = 16 bytes
            .set_iosqes(6); // Subm entry size: 2^6 = 64 bytes

        unsafe { self.write_reg(spec::CC, cfg.raw()) };

        cfg
    }

    fn select_command_set_selector(&self) -> u8 {
        if self.cap.css_none() {
            0b111
        } else if self.cap.css_some() {
            0b110
        } else {
            0b000
        }
    }

    fn enable(&mut self, cfg: &mut ControllerConfig) {
        cfg.set_enabled(true);
        unsafe { self.write_reg(spec::CC, cfg.raw()) };

        // wait for controller to enable
        while (unsafe { self.read_reg(spec::CSTS) } & 0x1) == 0 {}
    }

    pub fn create_io_subm_queue(
        &mut self,
        max_entries: usize,
        id: u16,
        comp_id: u16,
    ) -> Queue<Submission> {
        let size = max_entries * size_of::<SQEntry>();
        let pages = alloc_dma_region(size as u64);

        let mut entry = SQEntry::default();
        entry.cdw0 = spec::op::CRT_SUBQ | (1 << 16);
        entry.prp1 = pages.phys().as_u64();

        entry.cdw10 = (id as u32) | ((max_entries as u32 - 1) << 16);

        const PHYS_CONTIG: u32 = 1;
        entry.cdw11 = PHYS_CONTIG | ((comp_id as u32) << 16);

        println!("id: {id}, comp_id: {comp_id}");

        let res = self.submit_admin_command(entry);
        if !res.status.is_success() {
            panic!(
                "NVMe: Received status: {} whilst setting up I/O submission queue",
                res.status
            );
        }

        let mut queue = Queue::default();
        queue.region = Some(pages);
        queue.state.size = max_entries as u64;

        queue
    }

    pub fn create_io_comp_queue(
        &mut self,
        max_entries: usize,
        id: u16,
        vec: u32,
    ) -> Queue<Completion> {
        let size = max_entries * size_of::<CQEntry>();
        let pages = alloc_dma_region(size as u64);

        let mut entry = SQEntry::default();
        entry.cdw0 = spec::op::CRT_CMPQ | (1 << 16);
        entry.prp1 = pages.phys().as_u64();
        entry.cdw10 = (id as u32) | ((max_entries as u32 - 1) << 16);

        const COMPQUEUE_ENABLED: u32 = 0x2;
        const PHYS_CONTIG: u32 = 0x1;
        entry.cdw11 = PHYS_CONTIG | COMPQUEUE_ENABLED | (vec << 16);

        let res = self.submit_admin_command(entry);

        if !res.status.is_success() {
            panic!(
                "NVMe: Received status: {} whilst setting up I/O completion queue",
                res.status
            );
        }

        let mut queue = Queue::default();
        queue.region = Some(pages);
        queue.state.size = max_entries as u64;
        queue.id = id;

        queue
    }

    pub fn create_queue_pair(&mut self, entry_count: usize, id: u16) -> QueuePair {
        let comp = self.create_io_comp_queue(entry_count, id, id as u32);
        let subm = self.create_io_subm_queue(entry_count, id, id);

        QueuePair { subm, comp }
    }

    fn init_queue_cnt(&mut self) -> u16 {
        let io_queue_count_raw = self.set_features(spec::op::features::FID_NUM_QUEUES, |cmd| {
            cmd.cdw11 = ((spec::IO_QUEUES as u32 - 1) << 16) | (spec::IO_QUEUES as u32 - 1);
        });

        let io_comp_queues = (io_queue_count_raw.dw0 >> 16) as u16 + 1;
        let io_subm_queues = io_queue_count_raw.dw0 as u16 + 1;

        min(io_comp_queues, io_subm_queues).min(spec::IO_QUEUES)
    }

    pub fn nvme_int_handler(&self) -> IrqResult {
        println!("IRQ!");
        IrqResult::EoiNeeded
    }

    fn setup_interrupts(&mut self, self_ref: &Arc<Mutex<Self>>, queue_cnt: u32) {
        match self.setup_pci_interrupt_mode() {
            InterruptMode::MsiX => self.setup_msix_interrupts(self_ref, queue_cnt),
            InterruptMode::Msi => todo!(),
            InterruptMode::Legacy => todo!(),
        }
    }

    fn setup_msix_interrupts(&self, self_ref: &Arc<Mutex<Self>>, queue_cnt: u32) {
        let cpu_id = match &*INTERRUPT_CONTROLLER.get() {
            InterruptControllerType::Apic(apic_info) => apic_info.lapic.id(),
            _ => panic!("Using MSI-X, the interrupt controller should always be APIC!"),
        };

        let lock = self.device.lock();
        let mut map = lock
            .get_msix_tables()
            .expect("MSI-X tables should be present as MSI-X is enabled");

        for i in 0..queue_cnt as usize {
            let weak = Arc::downgrade(&self_ref);
            let vector = interrupts::allocate_interrupt(Box::new(move || {
                if let Some(ctrlr) = weak.upgrade() {
                    ctrlr.lock().nvme_int_handler()
                } else {
                    IrqResult::EoiNeeded
                }
            }))
            .expect("should be available interrupt vectors");

            map[i + 1].init(cpu_id, vector as u32);
        }
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
        identify.cdw0 = spec::op::IDENTIFY | (1 << 16);
        identify.prp1 = self.adm_buf.phys().as_u64();
        identify.cdw10 = cns;

        cmd(&mut identify);

        self.submit_admin_command(identify)
    }

    fn set_features(&mut self, fid: u32, cmd: impl FnOnce(&mut SQEntry)) -> CQEntry {
        let mut features = SQEntry::default();
        features.cdw0 = spec::op::SET_FEATURES | (1 << 16);
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

        // admin queues have id 0
        asq.id = 0;
        acq.id = 0;

        let aqa = ((acq.state.size as u32) << 16) | (asq.state.size as u32);
        unsafe { bar.write32(spec::AQA, aqa) };

        unsafe {
            bar.write64(spec::ASQ, asq.phys().unwrap());
            bar.write64(spec::ACQ, acq.phys().unwrap());
        }

        QueuePair {
            subm: asq,
            comp: acq,
        }
    }

    fn submit_admin_command(&mut self, cmd: SQEntry) -> CQEntry {
        let sq = &mut self.adm_queue.as_mut().unwrap().subm;
        let sq_id = sq.id;

        let slot = sq.virt().unwrap() + (sq.state.tail as u64 * size_of::<SQEntry>() as u64);
        unsafe {
            write_volatile(slot as *mut SQEntry, cmd);
        };

        sq.state.tail = (sq.state.tail + 1) % sq.state.size as u16;
        let tail = sq.state.tail;

        let doorbell = self.sq_doorbell(sq_id);
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

    pub fn css(&self) -> u8 {
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
