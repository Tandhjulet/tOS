use core::marker::PhantomData;

use crate::sys::mem::{
    addr::PhysAddr,
    page::{AddressNotAligned, PageSize, Size4KiB},
};

#[derive(Debug)]
pub struct PhysFrame<S: PageSize = Size4KiB> {
    start: PhysAddr,
    size: PhantomData<S>,
}

impl<S: PageSize> PhysFrame<S> {
    pub fn from_start_address(start: PhysAddr) -> Result<Self, AddressNotAligned> {
        if !start.is_aligned(S::SIZE) {
            return Err(AddressNotAligned);
        }

        Ok(unsafe { Self::from_start_address_unchecked(start) })
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
