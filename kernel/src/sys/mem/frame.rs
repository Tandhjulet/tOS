use core::marker::PhantomData;

use crate::sys::mem::{
    addr::PhysAddr,
    page::{PageSize, Size4KiB},
};

pub struct PhysFrame<S: PageSize = Size4KiB> {
    start: PhysAddr,
    size: PhantomData<S>,
}

impl<S: PageSize> PhysFrame<S> {
    pub fn containing_address(address: PhysAddr) -> Self {
        PhysFrame {
            start: address.align_down(S::SIZE),
            size: PhantomData,
        }
    }

    pub fn addr(&self) -> PhysAddr {
        self.start
    }
}
