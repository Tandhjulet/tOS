use spin::{Mutex, Once};

use crate::sys::mem::{
    frame::PhysFrame,
    page::{PageSize, Size4KiB},
    pmm::buddy::BuddyFrameAllocator,
};

pub mod boot;
pub mod buddy;

pub static PMM: Once<Mutex<BuddyFrameAllocator>> = Once::new();

pub fn alloc_page() -> Option<PhysFrame<Size4KiB>> {
    let pmm = PMM.wait();
    if let Some(allocator) = pmm {
        return allocator.lock().alloc_frame();
    }

    None
}

pub unsafe trait FrameAllocator<S: PageSize = Size4KiB> {
    fn alloc_frame(&mut self) -> Option<PhysFrame<S>>;
    fn dealloc_frame(&mut self, frame: PhysFrame<S>);

    fn alloc_frames(&mut self, count: usize) -> Option<PhysFrame<S>> {
        let start = self.alloc_frame()?;
        for _ in 1..count {
            self.alloc_frame()?;
        }

        Some(start)
    }
}
