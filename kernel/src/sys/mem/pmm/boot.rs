use bootloader_api::info::{MemoryRegion, MemoryRegionKind, MemoryRegions};

use crate::sys::mem::{
    FRAME_SIZE,
    frame::{PhysAddr, PhysFrame},
    page::Size4KiB,
    pmm::FrameAllocator,
};

pub struct BootFrameAllocator {
    memory_map: &'static [MemoryRegion],
    region_idx: usize,
    cursor: PhysAddr,
}

impl BootFrameAllocator {
    pub unsafe fn init(memory_map: &'static MemoryRegions) -> Self {
        BootFrameAllocator {
            memory_map,
            region_idx: 0,
            cursor: PhysAddr::zero(),
        }
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootFrameAllocator {
    fn alloc_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        while let Some(region) = self.memory_map.get(self.region_idx) {
            if region.kind != MemoryRegionKind::Usable {
                self.region_idx += 1;
                continue;
            }

            if self.cursor.as_u64() < region.start {
                self.cursor = PhysAddr::new(region.start);
            }

            let aligned = self.cursor.align_up(FRAME_SIZE);
            if aligned.as_u64() + FRAME_SIZE <= region.end {
                self.cursor = aligned + FRAME_SIZE;
                return Some(PhysFrame::containing_address(aligned));
            }

            self.region_idx += 1;
            self.cursor = PhysAddr::zero();
        }

        None
    }

    fn dealloc_frame(&mut self, _: PhysFrame<Size4KiB>) {
        panic!("boot allocator cannot deallocate");
    }
}
