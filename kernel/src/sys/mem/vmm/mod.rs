use core::marker::PhantomData;

use x86_64::VirtAddr;

use crate::sys::mem::page::{PageSize, Size4KiB};

pub mod mapper;
pub(super) mod paging;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(C)]
pub struct Page<S: PageSize = Size4KiB> {
    start_addr: VirtAddr,
    size: PhantomData<S>,
}

impl<S: PageSize> Page<S> {}
