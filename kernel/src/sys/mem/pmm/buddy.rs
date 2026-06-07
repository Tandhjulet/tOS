use crate::{
    allocator::mmio::PAGE_SIZE,
    core::{align::align_up, bitmap::Bitmap},
    sys::mem::{
        addr::VirtAddr,
        frame::PhysFrame,
        page::Size4KiB,
        pmm::{FrameAllocator, boot::BootFrameAllocator},
    },
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

impl BuddyFrameAllocator {
    pub fn bitmap_bits_at_order(total_frames: usize, order: usize) -> usize {
        total_frames.div_ceil(1usize << order)
    }

    pub fn bitmap_bytes_at_order(total_frames: usize, order: usize) -> usize {
        Self::bitmap_bits_at_order(total_frames, order).div_ceil(8)
    }

    pub fn total_bitmap_bytes(total_frames: usize) -> usize {
        (0..=MAX_ORDER)
            .map(|o| Self::bitmap_bytes_at_order(total_frames, o))
            .sum()
    }

    pub fn bitmap_frames_needed(total_frames: usize) -> usize {
        let bytes = Self::total_bitmap_bytes(total_frames);
        align_up(bytes, PAGE_SIZE as usize)
    }

    pub unsafe fn new(boot: &mut BootFrameAllocator, phys_offset: VirtAddr) {
        let total_frames = boot.usable_frame_count();
        let bitmap_frame_count = Self::bitmap_frames_needed(total_frames);

        let bitmap_region_start = boot
            .alloc_frames(bitmap_frame_count)
            .expect("should be able to initialize buddy allocator")
            .addr();
    }
}

unsafe impl FrameAllocator<Size4KiB> for BuddyFrameAllocator {
    fn alloc_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        todo!()
    }

    fn dealloc_frame(&mut self, frame: PhysFrame<Size4KiB>) {
        todo!()
    }
}
