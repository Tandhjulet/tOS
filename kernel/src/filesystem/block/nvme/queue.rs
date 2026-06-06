use core::{
    fmt::Display,
    marker::PhantomData,
    ptr::{read_volatile, write_volatile},
};

use alloc::sync::Weak;
use spin::Mutex;

use crate::{
    allocator::mmio::MappedRegion,
    filesystem::block::nvme::{NvmeController, commands::NvmeCommand},
};

pub trait QueueKind {
    fn doorbell(queue_id: u16, dstrd: u8) -> u32;
}

#[derive(Default)]
pub struct Submission;

#[derive(Default)]
pub struct Completion;

impl QueueKind for Submission {
    fn doorbell(queue_id: u16, dstrd: u8) -> u32 {
        0x1000 + (2 * queue_id as u32) * (4 << dstrd as u32)
    }
}
impl QueueKind for Completion {
    fn doorbell(queue_id: u16, dstrd: u8) -> u32 {
        0x1000 + (2 * queue_id as u32 + 1) * (4 << dstrd as u32)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RingQueueState {
    pub size: u64,

    pub tail: u16,
    pub head: u16,
    pub phase: bool,
}

impl RingQueueState {
    pub fn new(size: u64) -> Self {
        let mut state = RingQueueState::default();
        state.size = size;
        state
    }
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

pub trait QueuePairKind {
    fn submit_async(pair: &mut QueuePair<Self>, command: SQEntry) -> impl Future<Output = CQEntry>
    where
        Self: Sized;
}

pub struct Admin {
    pub buf: MappedRegion,
}

impl Admin {
    pub fn submit_cmd(pair: &mut QueuePair<Self>, command: SQEntry) {
        let slot =
            pair.subm.virt().unwrap() + (pair.subm.state.tail as u64 * size_of::<SQEntry>() as u64);
        unsafe { write_volatile(slot as *mut SQEntry, command) }

        pair.subm.state.tail = (pair.subm.state.tail + 1) % pair.subm.state.size as u16;
        let tail = pair.subm.state.tail;

        let doorbell = pair.subm.doorbell;
        unsafe { pair.write_reg(doorbell, tail as u32) }
    }

    pub fn next_completion(pair: &mut QueuePair<Self>) -> CQEntry {
        let (slot, phase) = {
            let cq = &pair.comp;
            let slot = cq.virt().unwrap() + (cq.state.head as u64 * size_of::<CQEntry>() as u64);
            (slot, cq.state.phase)
        };

        let entry = loop {
            let entry = unsafe { read_volatile(slot as *const CQEntry) };
            if entry.status.phase_tag() == phase {
                break entry;
            }
        };

        let new_head = {
            let cq = &mut pair.comp;
            cq.state.head += 1;
            if cq.state.head > cq.state.size as u16 {
                cq.state.head = 0;
                cq.state.phase = !cq.state.phase; // flip phase on wraparound
            }

            cq.state.head
        };

        let doorbell = pair.comp.doorbell;
        unsafe { pair.write_reg(doorbell, new_head as u32) }

        entry
    }

    pub fn submit_polled(pair: &mut QueuePair<Self>, command: SQEntry) -> CQEntry {
        Self::submit_cmd(pair, command);
        Self::next_completion(pair)
    }
}

pub struct Io;

impl QueuePairKind for Admin {
    fn submit_async(pair: &mut QueuePair<Self>, command: SQEntry) -> impl Future<Output = CQEntry>
    where
        Self: Sized,
    {
        let cq = Self::submit_polled(pair, command);
        core::future::ready(cq)
    }
}

impl QueuePairKind for Io {
    fn submit_async(pair: &mut QueuePair<Self>, command: SQEntry) -> impl Future<Output = CQEntry>
    where
        Self: Sized,
    {
        todo!()
    }
}

impl QueuePair<Io> {
    pub fn new_io(
        controller: Weak<Mutex<NvmeController>>,
        subm: Queue<Submission>,
        comp: Queue<Completion>,
    ) -> Self {
        QueuePair {
            controller,
            subm,
            comp,
            kind: Io,
        }
    }
}

impl QueuePair<Admin> {
    pub fn new_admin(
        controller: Weak<Mutex<NvmeController>>,
        subm: Queue<Submission>,
        comp: Queue<Completion>,
        buffer: MappedRegion,
    ) -> Self {
        QueuePair {
            controller,
            subm,
            comp,
            kind: Admin { buf: buffer },
        }
    }

    pub fn submit_sync<C: NvmeCommand>(&mut self, command: C) -> CQEntry {
        Admin::submit_polled(self, Self::build_entry(command))
    }

    pub unsafe fn read<T: Copy>(&self) -> T {
        unsafe { *(self.kind.buf.as_ptr::<T>()) }
    }

    pub fn buffer(&self) -> &MappedRegion {
        &self.kind.buf
    }
}

pub struct QueuePair<K: QueuePairKind> {
    pub controller: Weak<Mutex<NvmeController>>,
    pub subm: Queue<Submission>,
    pub comp: Queue<Completion>,
    pub kind: K,
}

impl<K: QueuePairKind> QueuePair<K> {
    fn build_entry<C: NvmeCommand>(command: C) -> SQEntry {
        let mut entry = SQEntry::default();
        entry.cdw0 = C::OPCODE | (1 << 16);
        command.configure(&mut entry);
        entry
    }

    pub fn submit<C: NvmeCommand>(&mut self, command: C) -> impl Future<Output = CQEntry> {
        K::submit_async(self, Self::build_entry(command))
    }

    unsafe fn write_reg(&mut self, offset: u32, val: u32) {
        if let Some(controller) = self.controller.upgrade() {
            unsafe { controller.lock().write_reg(offset, val) }
        }
    }
}

pub struct Queue<K: QueueKind> {
    pub id: u16,
    pub region: Option<MappedRegion>,
    pub state: RingQueueState,
    pub doorbell: u32,
    _phantom: PhantomData<K>,
}

impl<K: QueueKind> Queue<K> {
    pub fn new(
        id: u16,
        region: Option<MappedRegion>,
        state: RingQueueState,
        dstrd: u8,
    ) -> Queue<K> {
        let doorbell = K::doorbell(id, dstrd);

        Queue {
            id,
            region,
            state,
            doorbell,
            _phantom: PhantomData,
        }
    }

    pub fn phys(&self) -> Option<u64> {
        self.region.as_ref().map(|r| r.phys().as_u64())
    }

    pub fn virt(&self) -> Option<u64> {
        self.region.as_ref().map(|r| r.virt().as_u64())
    }
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
