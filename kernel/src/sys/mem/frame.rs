use core::marker::PhantomData;

use crate::sys::mem::{
    addr::PhysAddr,
    page_size::{PageSize, Size4KiB},
};

#[derive(Debug)]
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

    pub fn start_addr(&self) -> PhysAddr {
        self.start
    }
}
