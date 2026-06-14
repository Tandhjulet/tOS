use core::ops::Add;

use crate::{
    allocator::mmio::PAGE_SIZE,
    core::{align::align_up, bitmap::Bitmap},
    sys::mem::{
        addr::{PhysAddr, VirtAddr},
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

    pub unsafe fn new(boot: &mut BootFrameAllocator, phys_offset: VirtAddr) -> self {
        let total_frames = boot.usable_frame_count();
        let bitmap_frame_count = Self::bitmap_frames_needed(total_frames);

        let bitmap_region_start = boot
            .alloc_frames(bitmap_frame_count)
            .expect("should be able to initialize buddy allocator")
            .start_addr();

        // build bitmaps
        let mut bitmaps: [Option<Bitmap>; MAX_ORDER + 1] = [const { None }; MAX_ORDER + 1];
        let mut byte_offset = 0u64;
        for order in 0..=MAX_ORDER {
            let byte_len = Self::bitmap_bytes_at_order(total_frames, order);
            let blocks = Self::bitmap_bits_at_order(total_frames, order);
            if byte_len > 0 {
                bitmaps[order] = Some(Bitmap::from_raw(
                    bitmap_region_start.add(byte_offset).as_u64() as *mut u8,
                    byte_len,
                    blocks,
                ));
            }
            byte_offset += byte_len as u64;
        }

        let mut allocator = BuddyFrameAllocator {
            bitmaps,
            phys_offset,
            total_frames,
        };

        for frame in boot.usable_frames() {
            let frame_idx = (frame.start_addr().as_u64() / PAGE_SIZE) as usize;
            if frame_idx < total_frames {
                allocator.free_order(frame_idx, 0);
            }
        }

        // TODO: mark bitmap frames allocated

        allocator
    }

    pub fn alloc_order(&mut self, order: usize) -> Option<PhysFrame> {
        if order > MAX_ORDER {
            return None;
        }

        // any blocks free at this order?
        if let Some(block_idx) = self.bitmaps[order].as_ref()?.first_empty() {
            self.bitmaps[order].as_mut()?.clear(block_idx);
            let frame_idx = block_idx << order;
            return Some(PhysFrame::containing_address(PhysAddr::new(
                frame_idx as u64 * PAGE_SIZE,
            )));
        }

        // no? let's split upwards, then
        let parent = self.alloc_order(order + 1)?;
        let parent_frame_idx = (parent.start_addr().as_u64() / PAGE_SIZE) as usize;
        let buddy_frame_idx = parent_frame_idx + (1 << order);

        if buddy_frame_idx < self.total_frames {
            let bitmap_idx = buddy_frame_idx >> order;
            self.bitmaps[order].as_mut()?.set(bitmap_idx);
        }

        Some(parent)
    }

    pub fn free_order(&mut self, frame_idx: usize, order: usize) {
        if order > MAX_ORDER {
            return;
        }

        let block_idx = frame_idx >> order;
        let buddy_idx = block_idx ^ 1;

        let buddy_free = buddy_idx < Self::bitmap_bits_at_order(self.total_frames, order)
            && self.bitmaps[order]
                .as_ref()
                .map_or(false, |b| b.get(buddy_idx));

        if buddy_free {
            self.bitmaps[order].as_mut().map(|b| b.clear(buddy_idx));
            let parent_idx = (block_idx & !1) << order;
            self.free_order(parent_idx, order + 1);
        } else {
            self.bitmaps[order].as_mut().map(|b| b.set(block_idx));
        }
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
