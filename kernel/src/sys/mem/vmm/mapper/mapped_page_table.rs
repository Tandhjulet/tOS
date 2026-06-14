use crate::sys::mem::{
    addr::VirtAddr,
    frame::PhysFrame,
    page::{PageSize, Size1GiB, Size2MiB, Size4KiB},
    pmm::FrameAllocator,
    vmm::{
        Page,
        mapper::{MapError, Mapper, TranslateError, UnmapError},
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
        let p1 = self.page_table_walker.create_walk_to_mut(
            self.level_4_table,
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

    fn unmap(&mut self, page: Page<Size4KiB>) -> Result<PhysFrame, super::UnmapError> {
        let p1 = self.page_table_walker.walk_to_mut(
            self.level_4_table,
            &[page.p4_index(), page.p3_index(), page.p2_index()],
        )?;

        let entry = &mut p1[page.p1_index()];

        let frame = entry.frame().map_err(|err| match err {
            FrameError::FrameNotPresent => UnmapError::PageNotMapped,
            FrameError::HugeFrame => UnmapError::ParentHugePage,
        })?;

        entry.clear();
        Ok(frame)
    }

    fn translate_page(
        &self,
        page: Page<Size4KiB>,
    ) -> Result<PhysFrame<Size4KiB>, super::TranslateError> {
        let p1 = self.page_table_walker.walk_to(
            self.level_4_table,
            &[page.p4_index(), page.p3_index(), page.p2_index()],
        )?;

        let entry = &p1[page.p1_index()];
        if entry.is_unused() {
            return Err(TranslateError::PageNotMapped);
        }

        let frame: PhysFrame = entry.frame().map_err(|err| match err {
            FrameError::FrameNotPresent => TranslateError::PageNotMapped,
            FrameError::HugeFrame => TranslateError::ParentHugePage,
        })?;

        Ok(frame)
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
        let p2 = self.page_table_walker.create_walk_to_mut(
            self.level_4_table,
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

    fn unmap(&mut self, page: Page<Size2MiB>) -> Result<PhysFrame<Size2MiB>, super::UnmapError> {
        todo!()
    }

    fn translate_page(
        &self,
        page: Page<Size2MiB>,
    ) -> Result<PhysFrame<Size2MiB>, super::TranslateError> {
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
        let p3 = self.page_table_walker.create_walk_to_mut(
            self.level_4_table,
            &[page.p4_index()],
            parent_flags,
            frame_allocator,
        )?;

        let entry = &mut p3[page.p3_index()];
        if !entry.is_unused() {
            return Err(MapError::AlreadyMapped(frame));
        }

        entry.set_addr_and_flags(frame.start_addr(), flags | PageTableFlags::HUGE_PAGE);
        Ok(())
    }

    fn unmap(&mut self, page: Page<Size1GiB>) -> Result<PhysFrame<Size1GiB>, super::UnmapError> {
        todo!()
    }

    fn translate_page(
        &self,
        page: Page<Size1GiB>,
    ) -> Result<PhysFrame<Size1GiB>, super::TranslateError> {
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

    fn next_table<'b>(
        &self,
        entry: &'b PageTableEntry,
    ) -> Result<&'b PageTable, PageTableWalkError> {
        let table_ptr = self.next_table_ptr(entry.frame()?);
        let page_table = unsafe { &*table_ptr };

        Ok(page_table)
    }

    fn next_mut_table<'b>(
        &self,
        entry: &'b mut PageTableEntry,
    ) -> Result<&'b mut PageTable, PageTableWalkError> {
        let table_ptr = self.next_table_ptr(entry.frame()?);
        let page_table = unsafe { &mut *table_ptr };

        Ok(page_table)
    }

    fn create_next_table<'b>(
        &self,
        entry: &'b mut PageTableEntry,
        insert_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<&'b mut PageTable, PageTableCreateError> {
        let created: bool;

        if entry.is_unused() {
            let frame = frame_allocator
                .alloc_frame()
                .ok_or(PageTableCreateError::FrameAllocationFailed)?;

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
            Err(PageTableWalkError::MappedToHugePage) => {
                return Err(PageTableCreateError::MappedToHugePage);
            }
            Err(PageTableWalkError::NotMapped) => panic!("entry should be mapped at this point"),
        };

        if created {
            page_table.zero();
        }

        Ok(page_table)
    }

    fn walk_to<'b>(
        &self,
        root: &'b PageTable,
        indices: &[PageTableIndex],
    ) -> Result<&'b PageTable, PageTableWalkError> {
        let mut current: *const PageTable = root;
        for &idx in indices {
            let entry = &(*root)[idx];
            current = self.next_table_ptr(entry.frame()?) as *const PageTable;
        }

        Ok(unsafe { &*current })
    }

    fn walk_to_mut<'b>(
        &self,
        root: &'b mut PageTable,
        indices: &[PageTableIndex],
    ) -> Result<&'b mut PageTable, PageTableWalkError> {
        let mut current: *mut PageTable = root;
        for &idx in indices {
            let entry = &(*root)[idx];
            current = self.next_table_ptr(entry.frame()?) as *mut PageTable;
        }

        Ok(unsafe { &mut *current })
    }

    fn create_walk_to_mut<'b>(
        &self,
        root: &'b mut PageTable,
        indices: &[PageTableIndex],
        parent_flags: PageTableFlags,
        frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    ) -> Result<&'b mut PageTable, PageTableCreateError> {
        let mut current: *mut PageTable = root;
        for &idx in indices {
            let entry = &mut (*root)[idx];
            current = self.create_next_table(entry, parent_flags, frame_allocator)?;
        }

        Ok(unsafe { &mut *current })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageTableCreateError {
    FrameAllocationFailed,
    MappedToHugePage,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageTableWalkError {
    MappedToHugePage,
    NotMapped,
}

impl From<PageTableWalkError> for TranslateError {
    #[inline]
    fn from(err: PageTableWalkError) -> Self {
        match err {
            PageTableWalkError::MappedToHugePage => TranslateError::ParentHugePage,
            PageTableWalkError::NotMapped => TranslateError::PageNotMapped,
        }
    }
}

impl From<FrameError> for PageTableWalkError {
    #[inline]
    fn from(err: FrameError) -> Self {
        match err {
            FrameError::HugeFrame => PageTableWalkError::MappedToHugePage,
            FrameError::FrameNotPresent => PageTableWalkError::NotMapped,
        }
    }
}

impl From<PageTableWalkError> for UnmapError {
    fn from(err: PageTableWalkError) -> Self {
        match err {
            PageTableWalkError::MappedToHugePage => UnmapError::ParentHugePage,
            PageTableWalkError::NotMapped => UnmapError::PageNotMapped,
        }
    }
}

impl<S: PageSize> From<PageTableCreateError> for MapError<S> {
    fn from(err: PageTableCreateError) -> Self {
        match err {
            PageTableCreateError::FrameAllocationFailed => MapError::FrameAllocationFailed,
            PageTableCreateError::MappedToHugePage => MapError::ParentHugePage,
        }
    }
}
