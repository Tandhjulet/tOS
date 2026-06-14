use core::marker::PhantomData;

use crate::sys::mem::{
    addr::VirtAddr,
    page::{PageSize, Size4KiB},
    vmm::paging::PageTableIndex,
};

pub mod mapper;
pub(super) mod paging;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
#[repr(C)]
pub struct Page<S: PageSize = Size4KiB> {
    start_addr: VirtAddr,
    size: PhantomData<S>,
}

impl<S: PageSize> Page<S> {
    pub const SIZE: u64 = S::SIZE;

    pub const fn start_address(&self) -> VirtAddr {
        self.start_addr
    }

    pub const fn size(self) -> u64 {
        S::SIZE
    }

    pub(super) const fn p4_index(&self) -> PageTableIndex {
        self.start_address().pml4()
    }

    pub(super) const fn p3_index(&self) -> PageTableIndex {
        self.start_address().pdpt()
    }

    pub(super) const fn p2_index(&self) -> PageTableIndex {
        self.start_address().pd()
    }

    pub(super) const fn p1_index(&self) -> PageTableIndex {
        self.start_address().pt()
    }
}
