use core::{cell::UnsafeCell, sync::atomic::AtomicUsize};

use crate::sync::Lock;

pub struct SeqLock<T> {
    seq: AtomicUsize,
    data: UnsafeCell<T>,
}

impl<T> SeqLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            seq: AtomicUsize::new(0),
            data: UnsafeCell::new(data),
        }
    }
}

unsafe impl<T> Lock<T> for SeqLock<T> {
    unsafe fn get_data_unchecked(&self) -> *mut T {
        self.data.get()
    }

    unsafe fn unlock_from_reader(&self) {
        todo!()
    }

    unsafe fn unlock_from_writer(&self) {
        todo!()
    }
}
