use core::{
    fmt::Display,
    marker::PhantomData,
    ptr::{read_volatile, write_volatile},
};

use alloc::sync::Arc;

use crate::{allocator::mmio::MappedRegion, filesystem::block::nvme::NvmeController};

pub trait QueueKind {}

#[derive(Default)]
pub struct Submission;
#[derive(Default)]
pub struct Completion;

impl QueueKind for Submission {}
impl QueueKind for Completion {}

#[derive(Debug)]
pub struct RingQueueState {
    pub size: u64,

    pub tail: u16,
    pub head: u16,
    pub phase: bool,
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

pub struct QueuePair {
    pub controller: Arc<NvmeController>,
    pub subm: Queue<Submission>,
    pub comp: Queue<Completion>,
}

impl QueuePair {
    pub fn submit_polled(&mut self, command: SQEntry) -> CQEntry {
        self.submit_cmd(command);
        self.next_completion()
    }

    pub fn submit_cmd(&mut self, command: SQEntry) {
        let slot =
            self.subm.virt().unwrap() + (self.subm.state.tail as u64 * size_of::<SQEntry>() as u64);
        unsafe {
            write_volatile(slot as *mut SQEntry, command);
        };

        self.subm.state.tail = (self.subm.state.tail + 1) % self.subm.state.size as u16;
        let tail = self.subm.state.tail;

        let doorbell = self.controller.sq_doorbell(self.subm.id);
        unsafe {
            self.controller.write_reg(doorbell, tail as u32);
        };
    }

    pub fn next_completion(&mut self) -> CQEntry {
        let (slot, phase) = {
            let cq = &self.comp;
            let slot = cq.virt().unwrap() + (cq.state.head as u64 * size_of::<CQEntry>() as u64);
            (slot, cq.state.phase)
        };

        let entry = loop {
            let entry = unsafe { read_volatile(slot as *const CQEntry) };
            if entry.status.phase_tag() == phase {
                break entry;
            }
        };

        let doorbell = self.controller.cq_doorbell(self.comp.id);
        let new_head = {
            let cq = &mut self.comp;
            cq.state.head += 1;
            if cq.state.head > cq.state.size as u16 {
                cq.state.head = 0;
                cq.state.phase = !cq.state.phase; // flip phase on wraparound
            }

            cq.state.head
        };

        unsafe { self.controller.write_reg(doorbell, new_head as u32) };

        entry
    }
}

#[derive(Default)]
pub struct Queue<K: QueueKind> {
    pub id: u16,
    pub region: Option<MappedRegion>,
    pub state: RingQueueState,
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

impl Display for Status {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "0b{:016b} (type={}, code={}, success={})",
            self.0,
            self.code_type(),
            self.code(),
            self.is_success()
        )
    }
}
