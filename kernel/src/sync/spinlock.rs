use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::sync::{Lock, WriteLockGuard};

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

impl<T> SpinLock<T> {
    pub const fn new(data: T) -> Self {
        SpinLock {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    pub fn lock(&self) -> WriteLockGuard<'_, SpinLock<T>, T> {
        loop {
            if let Some(guard) = self.try_lock() {
                return guard;
            }

            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    pub fn try_lock(&self) -> Option<WriteLockGuard<'_, SpinLock<T>, T>> {
        if self.locked.swap(true, Ordering::Acquire) {
            return None;
        }

        Some(WriteLockGuard {
            lock: self,
            _data: PhantomData,
        })
    }

    unsafe fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

unsafe impl<T> Lock<T> for SpinLock<T> {
    unsafe fn get_data_unchecked(&self) -> *mut T {
        unsafe { &mut *self.data.get() }
    }

    unsafe fn unlock_from_reader(&self) {
        unsafe { self.unlock() }
    }

    unsafe fn unlock_from_writer(&self) {
        unsafe { self.unlock() }
    }
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
