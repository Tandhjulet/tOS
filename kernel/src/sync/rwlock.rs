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
    writer_wake_count: AtomicU32,
    // Every reader is +2, if a writer is waiting +1
    // u32::MAX if a writer is currently holding the lock
    state: AtomicU32,
    data: UnsafeCell<T>,

    _relax: PhantomData<R>,
}

impl<T, R: RelaxStrategy> RwLock<T, R> {
    pub const fn new(data: T) -> Self {
        Self {
            writer_wake_count: AtomicU32::new(0),
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

    pub fn write(&self) -> WriteLockGuard<'_, RwLock<T, R>, T> {
        let mut s = self.state.load(Ordering::Relaxed);
        loop {
            // try to acquire the lock
            if s <= 1 {
                match self
                    .state
                    .compare_exchange(s, u32::MAX, Ordering::Acquire, Ordering::Relaxed)
                {
                    Ok(_) => {
                        return WriteLockGuard {
                            lock: self,
                            _data: PhantomData,
                        };
                    }
                    Err(e) => {
                        s = e;
                        continue;
                    }
                }
            }

            // assure we don't acquire more readers to avoid writer-starvation
            if s % 2 == 0 {
                match self
                    .state
                    .compare_exchange(s, s + 1, Ordering::Relaxed, Ordering::Relaxed)
                {
                    Ok(_) => {}
                    Err(e) => {
                        s = e;
                        continue;
                    }
                }
            }

            // wait, if we can't acquire the lock
            let w = self.writer_wake_count.load(Ordering::Acquire);
            s = self.state.load(Ordering::Relaxed);
            if s >= 2 {
                R::relax(&self.writer_wake_count, w);
                s = self.state.load(Ordering::Relaxed);
            }
        }
    }
}

unsafe impl<T, R: RelaxStrategy> Lock<T> for RwLock<T, R> {
    unsafe fn get_data_unchecked(&self) -> *mut T {
        self.data.get()
    }

    unsafe fn unlock_from_reader(&self) {
        // if there are no active readers left AND we have an
        // awaiting writer, wake it up!
        if self.state.fetch_sub(2, Ordering::Release) == 3 {
            self.writer_wake_count.fetch_add(1, Ordering::Release);
            R::notify_one(&self.writer_wake_count);
        }
    }

    unsafe fn unlock_from_writer(&self) {
        self.state.store(0, Ordering::Release);
        R::notify_all(&self.state);

        self.writer_wake_count.fetch_add(1, Ordering::Release);
        R::notify_one(&self.writer_wake_count);
    }
}
