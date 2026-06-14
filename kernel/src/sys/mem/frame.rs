use core::marker::PhantomData;

use x86_64::structures::paging::page::AddressNotAligned;

use crate::sys::mem::{
    addr::PhysAddr,
    page::{PageSize, Size4KiB},
};

#[derive(Debug)]
pub struct PhysFrame<S: PageSize = Size4KiB> {
    start: PhysAddr,
    size: PhantomData<S>,
}

impl<S: PageSize> PhysFrame<S> {
    pub const fn from_start_address(start: PhysAddr) -> Result<Self, AddressNotAligned> {
        unimplemented!()
    }

    pub const unsafe fn from_start_address_unchecked(start: PhysAddr) -> Self {
        Self {
            start,
            size: PhantomData,
        }
    }

    pub fn containing_address(address: PhysAddr) -> Self {
        PhysFrame {
            start: address.align_down(S::SIZE),
            size: PhantomData,
        }
    }

    pub fn start_addr(&self) -> PhysAddr {
        self.start
    }
}
