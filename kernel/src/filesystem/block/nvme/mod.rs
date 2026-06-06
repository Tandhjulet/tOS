use core::{cmp::min, sync::atomic::AtomicUsize};

use alloc::{boxed::Box, format, sync::Arc, sync::Weak, vec::Vec};
use log::error;
use spin::Mutex;

use crate::{
    allocator::mmio::{PAGE_SIZE, alloc_dma_region},
    filesystem::block::{
        DeviceId, REGISTRY,
        nvme::{
            commands::{
                AmsType, ControllerCap, ControllerConfig, CreateCompletionQueueCommand,
                CreateSubmissionQueueCommand, IdentifyCommand, IdentifyCommandSet,
                IdentifyController, NvmeCommand, SetFeaturesCommand,
            },
            namespace::{
                IdentifyNamespaceIndependent, IdentifyNamespaceList, IdentifyNamespaceNvm,
                IdentifyNamespaceSpecificNvm, NvmNamespaceData, NvmeCommandSet, NvmeNamespace,
            },
            queue::{
                Admin, CQEntry, Completion, Io, Queue, QueuePair, RingQueueState, SQEntry,
                Submission,
            },
        },
    },
    io::pci::{PciDevice, bar::Bar},
    println,
    sys::interrupts::{
        self, INTERRUPT_CONTROLLER, InterruptControllerType, InterruptMode, IrqResult,
    },
};

pub mod commands;
pub mod namespace;
pub mod queue;
pub mod spec;

static NVME_COUNTER: AtomicUsize = AtomicUsize::new(0);

pub struct NvmeController {
    device: Arc<Mutex<PciDevice>>,
    cap: ControllerCap,

    identify_ctlr: Option<IdentifyController>,

    adm_queue: Option<QueuePair<Admin>>,

    pub queues: Vec<Arc<Mutex<QueuePair<Io>>>>,
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

            let weak = Arc::downgrade(this);
            let admin_queues = controller.create_admin_queues(Weak::clone(&weak));
            controller.adm_queue = Some(admin_queues);

            let mut cfg = controller.configure();
            controller.enable(&mut cfg);
            controller.run_identify_seq(&mut cfg);

            let queue_cnt = controller.init_queue_cnt() as u32;
            controller.create_io_queues(queue_cnt, weak);

            let namespaces = controller.enumerate_namespaces();
            namespaces
        };

        {
            let mut controller = this.lock();
            let queue_cnt = controller.queues.len();
            controller.setup_interrupts(queue_cnt as u32);
        };

        let mut registry = REGISTRY.lock();
        let nvme_id = NVME_COUNTER.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
        for ns in namespaces {
            let id = DeviceId(format!("nvme{}n{}", nvme_id, ns.nsid));

            let block_size = ns.command_set.block_size();
            let block_count = ns.command_set.block_count();

            registry.register(id, Arc::new(Mutex::new(ns)), block_size, block_count)
        }
    }

    fn create_io_queues(&mut self, queue_cnt: u32, controller: Weak<Mutex<NvmeController>>) {
        let slots_needed = queue_cnt as usize - self.queues.len();
        self.queues.reserve_exact(slots_needed);

        const ENTRY_COUNT: usize = PAGE_SIZE as usize / size_of::<SQEntry>();
        for i in 0..queue_cnt as u16 {
            let pair = self.create_queue_pair(ENTRY_COUNT, i + 1, Weak::clone(&controller));
            self.queues.push(pair);
        }
    }

    fn run_identify_seq(&mut self, cfg: &mut ControllerConfig) {
        let buffer = unsafe { self.admin_queue() }.buffer().phys().as_u64();
        let identify_ctrlr = unsafe {
            self.submit_read_admin_command::<IdentifyController>(IdentifyCommand {
                cns: spec::op::identify::CNS_CONTROLLER,
                prp: buffer,
                nsid: 0,
                cdw11: 0,
            })
        };

        self.identify_ctlr = Some(identify_ctrlr);

        if cfg.css() == 0 {
            // TODO
        }
    }

    fn enumerate_namespaces(&mut self) -> Vec<NvmeNamespace> {
        let mut namespaces = Vec::new();

        let buffer = unsafe { self.admin_queue() }.buffer().phys().as_u64();
        let cmd_set = unsafe {
            self.submit_read_admin_command::<IdentifyCommandSet>(IdentifyCommand {
                cns: spec::op::identify::CNS_CMD_SET,
                prp: buffer,
                cdw11: 0,
                nsid: 0,
            })
        };

        let selected_cmd_idx = cmd_set.first_valid().unwrap();

        // Refer to section 5.27.1.21 for documentation regarding
        // I/O Command Set Profile (FID: 0x19)
        unsafe {
            self.submit_admin_command(SetFeaturesCommand {
                fid: spec::op::features::FID_SET_PROFILE,
                cdw11: selected_cmd_idx as u32,
                prp: buffer,
            })
        };

        let mut ns_idx = 0;
        for csi in cmd_set.csi_iter(selected_cmd_idx) {
            let nsids = unsafe {
                self.submit_read_admin_command::<IdentifyNamespaceList>(IdentifyCommand {
                    cns: spec::op::identify::CNS_ACTIVE_NS_CMD_SET,
                    prp: buffer,
                    cdw11: (csi as u32) << 24,
                    nsid: 0,
                })
            };

            for &nsid in nsids.valid() {
                let queue = Arc::clone(&self.queues[ns_idx % self.queues.len()]);
                namespaces.push(self.build_namespace(nsid, csi, queue));

                ns_idx += 1;
            }
        }

        namespaces
    }

    fn build_namespace(
        &mut self,
        nsid: u32,
        csi: u8,
        queue: Arc<Mutex<QueuePair<Io>>>,
    ) -> NvmeNamespace {
        let buffer = unsafe { self.admin_queue() }.buffer().phys().as_u64();

        let command_set = match csi {
            spec::csi::NVM => {
                let identify = unsafe {
                    self.submit_read_admin_command::<IdentifyNamespaceNvm>(IdentifyCommand {
                        cns: spec::op::identify::CNS_NAMESPACE,
                        prp: buffer,
                        cdw11: 0,
                        nsid: nsid,
                    })
                };

                let specific = unsafe {
                    self.submit_read_admin_command::<IdentifyNamespaceSpecificNvm>(
                        IdentifyCommand {
                            cns: spec::op::identify::CNS_SPECIFIC_NS,
                            prp: buffer,
                            nsid: nsid,
                            cdw11: (csi as u32) << 24,
                        },
                    )
                };

                NvmeCommandSet::Nvm(NvmNamespaceData { identify, specific })
            }
            _ => todo!(),
        };

        unsafe {
            self.submit_admin_command(IdentifyCommand {
                cns: spec::op::identify::CNS_SPECIFIC_CTRLR,
                nsid: 0,
                prp: buffer,
                cdw11: (csi as u32) << 24,
            })
        };

        let independent = unsafe {
            self.submit_read_admin_command::<IdentifyNamespaceIndependent>(IdentifyCommand {
                cns: spec::op::identify::CNS_NAMESPACE_INDEPENDENT,
                nsid: nsid,
                prp: buffer,
                cdw11: 0,
            })
        };

        NvmeNamespace {
            queue,
            nsid,
            command_set,
            independent,
        }
    }

    fn reset_and_disable(&mut self) {
        let mut cfg = self.get_configuration();
        cfg.set_enabled(false);
        unsafe { self.write_reg(spec::CC, cfg.raw()) };

        // wait for controller to disable
        while (unsafe { self.read_reg(spec::CSTS) } & 0x1) == 1 {}
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
        const PHYS_CONTIG: u32 = 1;

        let size = max_entries * size_of::<SQEntry>();
        let pages = alloc_dma_region(size as u64);

        let res = unsafe {
            self.submit_admin_command(CreateSubmissionQueueCommand {
                prp: pages.phys().as_u64(),
                cdw10: (id as u32) | ((max_entries as u32 - 1) << 16),
                cdw11: PHYS_CONTIG | ((comp_id as u32) << 16),
            })
        };

        if !res.status.is_success() {
            panic!(
                "NVMe: Received status: {} whilst setting up I/O submission queue",
                res.status
            );
        }

        Queue::new(
            id,
            Some(pages),
            RingQueueState::new(max_entries as u64),
            self.cap.dstrd(),
        )
    }

    pub fn create_io_comp_queue(
        &mut self,
        max_entries: usize,
        id: u16,
        vec: u32,
    ) -> Queue<Completion> {
        const COMPQUEUE_ENABLED: u32 = 0x2;
        const PHYS_CONTIG: u32 = 0x1;

        let size = max_entries * size_of::<CQEntry>();
        let pages = alloc_dma_region(size as u64);

        let res = unsafe {
            self.submit_admin_command(CreateCompletionQueueCommand {
                prp: pages.phys().as_u64(),
                cdw10: (id as u32) | ((max_entries as u32 - 1) << 16),
                cdw11: PHYS_CONTIG | COMPQUEUE_ENABLED | (vec << 16),
            })
        };

        if !res.status.is_success() {
            panic!(
                "NVMe: Received status: {} whilst setting up I/O completion queue",
                res.status
            );
        }

        Queue::new(
            id,
            Some(pages),
            RingQueueState::new(max_entries as u64),
            self.cap.dstrd(),
        )
    }

    unsafe fn admin_queue(&mut self) -> &mut QueuePair<Admin> {
        self.adm_queue
            .as_mut()
            .expect("Admin queues should be initialized")
    }

    pub unsafe fn submit_admin_command(&mut self, command: impl NvmeCommand) -> CQEntry {
        unsafe { self.admin_queue() }.submit_sync(command)
    }

    pub unsafe fn submit_read_admin_command<T: Copy>(&mut self, command: impl NvmeCommand) -> T {
        unsafe {
            self.submit_admin_command(command);
            self.admin_queue().read::<T>()
        }
    }

    pub fn create_queue_pair(
        &mut self,
        entry_count: usize,
        id: u16,
        controller: Weak<Mutex<NvmeController>>,
    ) -> Arc<Mutex<QueuePair<Io>>> {
        let comp = self.create_io_comp_queue(entry_count, id, id as u32);
        let subm = self.create_io_subm_queue(entry_count, id, id);

        Arc::new(Mutex::new(QueuePair::<Io>::new_io(controller, subm, comp)))
    }

    fn init_queue_cnt(&mut self) -> u16 {
        let buffer = unsafe { self.admin_queue() }.buffer().phys().as_u64();
        let io_queue_count_raw = unsafe {
            self.submit_admin_command(SetFeaturesCommand {
                fid: spec::op::features::FID_NUM_QUEUES,
                prp: buffer,
                cdw11: ((spec::IO_QUEUES as u32 - 1) << 16) | (spec::IO_QUEUES as u32 - 1),
            })
        };

        let io_comp_queues = (io_queue_count_raw.dw0 >> 16) as u16 + 1;
        let io_subm_queues = io_queue_count_raw.dw0 as u16 + 1;

        min(io_comp_queues, io_subm_queues).min(spec::IO_QUEUES)
    }

    pub fn interrupt_handler(&self) -> IrqResult {
        println!("IRQ!");
        IrqResult::EoiNeeded
    }

    fn setup_interrupts(&mut self, queue_cnt: u32) {
        match self.setup_pci_interrupt_mode() {
            InterruptMode::MsiX => self.setup_msix_interrupts(queue_cnt),
            InterruptMode::Msi => todo!(),
            InterruptMode::Legacy => todo!(),
        }
    }

    fn setup_msix_interrupts(&self, queue_cnt: u32) {
        let cpu_id = match &*INTERRUPT_CONTROLLER.get() {
            InterruptControllerType::Apic(apic_info) => apic_info.lapic.id(),
            _ => panic!("Using MSI-X, the interrupt controller should always be APIC!"),
        };

        let lock = self.device.lock();
        let mut map = lock
            .get_msix_tables()
            .expect("MSI-X tables should be present as MSI-X is enabled");

        for i in 0..queue_cnt as usize {
            let queue = Arc::downgrade(&self.queues[i]);
            let vector = interrupts::allocate_interrupt(Box::new(move || {
                if let Some(queue) = queue.upgrade() {
                    queue.lock().handle_incomming()
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

    fn create_admin_queues(&self, controller: Weak<Mutex<NvmeController>>) -> QueuePair<Admin> {
        let binding = self.device.lock();
        let Some(bar) = PciDevice::get_bar(&binding, 0) else {
            panic!("Could not find BAR0 for NVMe!");
        };

        const ADM_QUEUE_ID: u16 = 0;
        const RING_QUEUE_SIZE: u64 = 63;
        let dstrd = self.cap.dstrd();

        let asq = Queue::new(
            ADM_QUEUE_ID,
            Some(alloc_dma_region(PAGE_SIZE)),
            RingQueueState::new(RING_QUEUE_SIZE),
            dstrd,
        );
        let acq = Queue::new(
            ADM_QUEUE_ID,
            Some(alloc_dma_region(PAGE_SIZE)),
            RingQueueState::new(RING_QUEUE_SIZE),
            dstrd,
        );

        let aqa = ((acq.state.size as u32) << 16) | (asq.state.size as u32);
        unsafe { bar.write32(spec::AQA, aqa) };

        unsafe {
            bar.write64(spec::ASQ, asq.phys().unwrap());
            bar.write64(spec::ACQ, acq.phys().unwrap());
        }

        QueuePair::<Admin>::new_admin(controller, asq, acq, alloc_dma_region(PAGE_SIZE))
    }
}
