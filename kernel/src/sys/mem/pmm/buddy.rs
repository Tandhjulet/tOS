use crate::{
    core::bitmap::Bitmap,
    sys::mem::{addr::VirtAddr, frame::PhysFrame, page::Size4KiB, pmm::FrameAllocator},
};

// Largest block order supported for the Buddy Allocator.
// That is, the largest block can be FRAME_SIZE*2^MAX_ORDER large.
// Order 10 = 4 MiB (4096*2^10)
const MAX_ORDER: usize = 10;

pub struct BuddyFrameAllocator {
    bitmaps: [Option<Bitmap<'static>>; MAX_ORDER + 1],
    total_frames: usize,
    phys_offset: VirtAddr,
}

unsafe impl FrameAllocator<Size4KiB> for BuddyFrameAllocator {
    fn alloc_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        todo!()
    }

    fn dealloc_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        todo!()
    }
}
