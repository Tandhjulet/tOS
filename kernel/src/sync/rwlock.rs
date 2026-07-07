use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
};

use crate::sync::{
    Lock, ReadLockGuard, WriteLockGuard,
    relax::{RelaxStrategy, Spin},
};

pub struct RwLock<T, R = Spin> {
    writer_ref_count: AtomicU32,
    // Every reader is +2, if a writer is waiting +1
    state: AtomicU32,
    data: UnsafeCell<T>,

    _relax: PhantomData<R>,
}

impl<T, R: RelaxStrategy> RwLock<T, R> {
    pub const fn new(data: T) -> Self {
        Self {
            writer_ref_count: AtomicU32::new(0),
            state: AtomicU32::new(0),
            data: UnsafeCell::new(data),
            _relax: PhantomData,
        }
    }

    pub fn read(&self) -> ReadLockGuard<'_, RwLock<T, R>, T> {
        let mut s = self.state.load(Ordering::Relaxed);

        loop {
            if s % 2 == 0 {
                // Even
                assert!(s != u32::MAX - 1, "too many readers");
                match self
                    .state
                    .compare_exchange(s, s + 2, Ordering::Acquire, Ordering::Relaxed)
                {
                    Ok(_) => {
                        return ReadLockGuard {
                            _data: PhantomData,
                            lock: self,
                        };
                    }
                    Err(e) => {
                        s = e;
                    }
                }
            }

            if s % 2 == 1 {
                // Odd
                R::relax(&self.state, s);
                s = self.state.load(Ordering::Relaxed);
            }
        }
    }

    pub fn write(&self) -> WriteLockGuard<'_, RwLock<T, R>, T> {}

    unsafe fn unlock(&self) {}
}

unsafe impl<T, R: RelaxStrategy> Lock<T> for RwLock<T, R> {
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
