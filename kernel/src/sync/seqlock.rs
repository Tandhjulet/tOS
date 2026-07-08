use core::{
    cell::UnsafeCell,
    marker::PhantomData,
    mem::MaybeUninit,
    ptr,
    sync::atomic::{AtomicU32, Ordering, fence},
};

use crate::sync::{
    Lock, WriteLockGuard,
    relax::{RelaxStrategy, Spin},
};

pub struct SeqLock<T, R = Spin> {
    seq: AtomicU32,
    data: UnsafeCell<T>,
    _relax: PhantomData<R>,
}

impl<T: Copy, R: RelaxStrategy> SeqLock<T, R> {
    pub const fn new(data: T) -> Self {
        Self {
            seq: AtomicU32::new(0),
            data: UnsafeCell::new(data),
            _relax: PhantomData,
        }
    }

    pub fn read(&self) -> T {
        let mut seq1;
        loop {
            seq1 = self.seq.load(Ordering::Acquire);
            if seq1 % 2 != 0 {
                R::relax(&self.seq, seq1);
                continue;
            }

            // The data may get concurrently modified, but the rust compiler
            // assumes no data races, which is not the case here - thus it has to be volatile.
            let data = unsafe { ptr::read_volatile(self.data.get() as *mut MaybeUninit<T>) };

            // Prevent the data read from above to be reordered to
            // AFTER we load the sequence num
            fence(Ordering::Acquire);

            let seq2 = self.seq.load(Ordering::Relaxed);
            if seq1 == seq2 {
                return unsafe { data.assume_init() };
            }
        }
    }

    pub fn write(&self) -> WriteLockGuard<'_, SeqLock<T, R>, T> {
        let mut s = self.seq.load(Ordering::Acquire);
        loop {
            // Uneven, another writer currently holds the lock!
            if s % 2 != 0 {
                R::relax(&self.seq, s);
                s = self.seq.load(Ordering::Relaxed);
                continue;
            }

            // No writer is holding, let's try to acquire it!
            match self
                .seq
                .compare_exchange(s, s + 1, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(e) => {
                    s = e;
                    continue;
                }
            }
        }

        WriteLockGuard {
            lock: self,
            _data: PhantomData,
        }
    }
}

unsafe impl<T, R> Lock<T> for SeqLock<T, R> {
    unsafe fn get_data_unchecked(&self) -> *mut T {
        self.data.get()
    }

    unsafe fn unlock_from_reader(&self) {
        unimplemented!("reads are copied, not behind a guard")
    }

    unsafe fn unlock_from_writer(&self) {
        self.seq.fetch_add(1, Ordering::Release);
    }
}
