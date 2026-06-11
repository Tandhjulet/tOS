use crate::sys::mem::{
    addr::PhysAddr,
    frame::PhysFrame,
    page::{PageSize, Size4KiB},
    pmm::FrameAllocator,
    vmm::{Page, paging::PageTableFlags},
};

pub trait Mapper<S: PageSize> {
    unsafe fn map_to(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        frame_allocator: impl FrameAllocator<Size4KiB>,
    ) {
        unimplemented!()
    }

    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<S>,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        parent_flags: PageTableFlags,
        frame_allocator: impl FrameAllocator<Size4KiB>,
    ) -> Result<(), MapError<S>>;

    fn unmap(&mut self, page: Page<S>) -> Result<(), UnmapError>;

    unsafe fn identify_map(
        &mut self,
        frame: PhysFrame<S>,
        flags: PageTableFlags,
        frame_allocator: impl FrameAllocator<Size4KiB>,
    ) -> Result<(), MapError<S>> {
        unimplemented!()
    }
}

#[derive(Debug)]
pub enum MapError<S: PageSize> {
    FrameAllocationFailed,
    ParentHugePage,
    AlreadyMapped(PhysFrame<S>),
}

pub enum UnmapError {
    ParentHugePage,
    PageNotMapped,
    InvalidFrameAddr(PhysAddr),
}
