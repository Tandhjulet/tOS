use core::arch::asm;

use x86_64::registers::control::Cr3;

use crate::sys::mem::{
    addr::{PhysAddr, VirtAddr},
    frame::PhysFrame,
    page::{PageSize, Size4KiB},
    pmm::FrameAllocator,
    vmm::{Page, paging::PageTableFlags},
};

pub mod mapped_page_table;

pub trait Mapper<S: PageSize> {
    unsafe fn map_to(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<MapperFlush<S>, MapError<S>> {
        let parent_table_flags =
            flags & (PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::USER);

        unsafe {
            self.map_to_with_table_flags(page, frame, flags, parent_table_flags, frame_allocator)
        }
    }

    fn translate_page(&self, page: Page<S>) -> Result<PhysFrame<S>, TranslateError>;

    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        parent_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<MapperFlush<S>, MapError<S>>;

    fn unmap(&mut self, page: Page<S>) -> Result<(PhysFrame<S>, MapperFlush<S>), UnmapError>;

    unsafe fn identity_map(
        &mut self,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<MapperFlush<S>, MapError<S>> {
        let page = Page::containing_address(VirtAddr::new(frame.start_addr().as_u64()));
        unsafe { self.map_to(page, frame, flags, frame_allocator) }
    }
}

#[derive(Debug)]
pub enum MapError<S: PageSize> {
    FrameAllocationFailed,
    ParentHugePage,
    AlreadyMapped(PhysFrame<S>),
}

#[derive(Debug)]
pub enum UnmapError {
    ParentHugePage,
    PageNotMapped,
    InvalidFrameAddress(PhysAddr),
}

#[derive(Debug)]
pub enum TranslateError {
    PageNotMapped,
    ParentHugePage,
    InvalidFrameAddress(PhysAddr),
}

#[derive(Debug)]
#[must_use = "TLB should be flushed due to Page Table changes"]
pub struct MapperFlush<S: PageSize>(Page<S>);

impl<S: PageSize> MapperFlush<S> {
    pub fn new(page: Page<S>) -> Self {
        Self(page)
    }

    pub fn flush(self) {
        let addr = self.0.start_address();
        unsafe {
            asm!("invlpg [{}]", in(reg) addr.as_u64(), options(nostack, preserves_flags));
        }
    }

    pub fn ignore(self) {}

    pub fn page(&self) -> &Page<S> {
        &self.0
    }
}

#[derive(Debug)]
#[must_use = "TLB should be flushed due to Page Table changes"]
pub struct MapperFlushAll();

impl MapperFlushAll {
    pub fn new() -> Self {
        Self()
    }

    pub fn flush(self) {
        let (frame, flags) = Cr3::read();
        unsafe { Cr3::write(frame, flags) }
    }

    pub fn ignore(self) {}
}
