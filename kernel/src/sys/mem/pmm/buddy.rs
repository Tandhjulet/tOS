use crate::sys::mem::{frame::PhysFrame, page::Size4KiB, pmm::FrameAllocator};

pub struct BuddyFrameAllocator {}

unsafe impl FrameAllocator<Size4KiB> for BuddyFrameAllocator {
    fn alloc_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        todo!()
    }

    fn dealloc_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        todo!()
    }
}
