use crate::sys::mem::{
    addr::VirtAddr,
    frame::PhysFrame,
    page::{PageSize, Size1GiB, Size2MiB, Size4KiB},
    pmm::FrameAllocator,
    vmm::{
        Page,
        mapper::{MapError, Mapper},
        paging::{FrameError, PageTable, PageTableEntry, PageTableFlags, PageTableIndex},
    },
};

pub struct MappedPageTable<'a> {
    level_4_table: &'a mut PageTable,
    page_table_walker: PageTableWalker,
}

impl<'a> MappedPageTable<'a> {
    pub const fn new(level_4_table: &'a mut PageTable, offset: VirtAddr) -> Self {
        Self {
            level_4_table,
            page_table_walker: PageTableWalker { offset },
        }
    }

    pub const fn level_4_table(&self) -> &PageTable {
        self.level_4_table
    }

    pub const fn level_4_table_mut(&mut self) -> &mut PageTable {
        self.level_4_table
    }

    pub const fn offset(&self) -> &VirtAddr {
        &self.page_table_walker.offset
    }

    fn walk<S: PageSize>(
        &mut self,
        indices: &[PageTableIndex],
        parent_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator,
    ) -> Result<&'a mut PageTable, MapError<S>> {
        let mut current: *mut PageTable = self.level_4_table;
        for &idx in indices {
            current = self.page_table_walker.create_next_table(
                unsafe { &mut (*current)[idx] },
                parent_flags,
                frame_allocator,
            )? as *mut PageTable;
        }

        Ok(unsafe { &mut *current })
    }
}

impl Mapper<Size4KiB> for MappedPageTable<'_> {
    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<Size4KiB>,
        frame: PhysFrame<Size4KiB>,
        flags: PageTableFlags,
        parent_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), super::MapError<Size4KiB>> {
        let p1 = self.walk(
            &[page.p4_index(), page.p3_index(), page.p2_index()],
            parent_flags,
            frame_allocator,
        )?;

        let entry = &mut p1[page.p1_index()];
        if !entry.is_unused() {
            return Err(MapError::AlreadyMapped(frame));
        }

        entry.set_frame(frame, flags);
        Ok(())
    }

    fn unmap(&mut self, page: Page<Size4KiB>) -> Result<(), super::UnmapError> {
        todo!()
    }
}

impl Mapper<Size2MiB> for MappedPageTable<'_> {
    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<Size2MiB>,
        frame: PhysFrame<Size2MiB>,
        flags: PageTableFlags,
        parent_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), MapError<Size2MiB>> {
        let p2 = self.walk(
            &[page.p4_index(), page.p3_index()],
            parent_flags,
            frame_allocator,
        )?;

        let entry = &mut p2[page.p2_index()];
        if !entry.is_unused() {
            return Err(MapError::AlreadyMapped(frame));
        }

        entry.set_addr_and_flags(frame.start_addr(), flags | PageTableFlags::HUGE_PAGE);
        Ok(())
    }

    fn unmap(&mut self, page: Page<Size2MiB>) -> Result<(), super::UnmapError> {
        todo!()
    }
}

impl Mapper<Size1GiB> for MappedPageTable<'_> {
    unsafe fn map_to_with_table_flags(
        &mut self,
        page: Page<Size1GiB>,
        frame: PhysFrame<Size1GiB>,
        flags: PageTableFlags,
        parent_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<(), MapError<Size1GiB>> {
        let p3 = self.walk(&[page.p4_index()], parent_flags, frame_allocator)?;

        let entry = &mut p3[page.p3_index()];
        if !entry.is_unused() {
            return Err(MapError::AlreadyMapped(frame));
        }

        entry.set_addr_and_flags(frame.start_addr(), flags | PageTableFlags::HUGE_PAGE);
        Ok(())
    }

    fn unmap(&mut self, page: Page<Size1GiB>) -> Result<(), super::UnmapError> {
        todo!()
    }
}

struct PageTableWalker {
    offset: VirtAddr,
}

impl PageTableWalker {
    fn next_table_ptr(&self, frame: PhysFrame) -> *mut PageTable {
        let raw_table_ptr = self.offset + frame.start_addr().as_u64();
        raw_table_ptr.as_mut_ptr::<PageTable>()
    }

    fn next_table<'b>(&self, entry: PageTableEntry) -> Result<&'b PageTable, FrameError> {
        let table_ptr = self.next_table_ptr(entry.frame()?);
        let page_table = unsafe { &*table_ptr };

        Ok(page_table)
    }

    fn next_mut_table<'b>(
        &self,
        entry: &'b mut PageTableEntry,
    ) -> Result<&'b mut PageTable, FrameError> {
        let table_ptr = self.next_table_ptr(entry.frame()?);
        let page_table = unsafe { &mut *table_ptr };

        Ok(page_table)
    }

    fn create_next_table<'b>(
        &self,
        entry: &'b mut PageTableEntry,
        insert_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<&'b mut PageTable, MapError<_>> {
        let created: bool;

        if entry.is_unused() {
            let frame = frame_allocator
                .alloc_frame()
                .ok_or(MapError::FrameAllocationFailed)?;

            entry.set_frame(frame, insert_flags);
            created = true;
        } else {
            if !insert_flags.empty() && !entry.flags().contains(insert_flags) {
                entry.set_flags(entry.flags() | insert_flags);
            }

            created = false;
        }

        let page_table = match self.next_mut_table(entry) {
            Ok(table) => table,
            Err(FrameError::HugeFrame) => {
                return Err(MapError::ParentHugePage);
            }
            Err(FrameError::FrameNotPresent) => panic!("entry should be mapped at this point"),
        };

        if created {
            page_table.zero();
        }

        Ok(page_table)
    }
}

pub enum PageTableCreateError {
    FrameAllocationFailed,
    MappedToHugePage,
}
