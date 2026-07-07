use core::{cell::UnsafeCell, sync::atomic::AtomicU32};

pub struct RwLock<T> {
    // Every reader enqueued is mult. by two (+2)
    // Every writer enqueued is uneven
    // U32::MAX => writer is active
    ref_count: AtomicU32,
    data: UnsafeCell<T>,
}

impl<T> RwLock<T> {
    pub const fn new(data: T) -> Self {
        Self {
            ref_count: AtomicU32::new(0),
            data: UnsafeCell::new(data),
        }
    }

    pub fn read() {}
}

pub struct ReaderGuard<T> {
    lock: T,
}

pub struct WriterGuard<T> {
    lock: T,
}
