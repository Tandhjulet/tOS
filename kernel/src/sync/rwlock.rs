use core::{cell::UnsafeCell, sync::atomic::AtomicU32};

use crate::sync::{Lock, ReadLockGuard, WriteLockGuard};

pub struct RwLock<T> {
    writer_ref_count: AtomicU32,
    // Every reader is +2, if a writer is waiting +1
    state: AtomicU32,
    data: UnsafeCell<T>,
}

impl<T> RwLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            writer_ref_count: AtomicU32::new(0),
            state: AtomicU32::new(0),
            data: UnsafeCell::new(data),
        }
    }

    pub fn read(&self) -> ReadLockGuard<'_, RwLock<T>, T> {
        todo!()
    }

    pub fn write(&self) -> WriteLockGuard<'_, RwLock<T>, T> {
        todo!()
    }

    unsafe fn unlock(&self) {}
}

unsafe impl<T> Lock<T> for RwLock<T> {
    unsafe fn get_data_unchecked(&self) -> *mut T {
        unsafe { &mut *self.data.get() }
    }

    unsafe fn unlock_from_reader(&self) {
        todo!()
    }

    unsafe fn unlock_from_writer(&self) {
        todo!()
    }
}
