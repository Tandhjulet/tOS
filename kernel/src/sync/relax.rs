use core::sync::atomic::AtomicU32;

pub trait RelaxStrategy {
    fn relax(atomic: &AtomicU32, expected: u32);

    fn notify_one(atomic: &AtomicU32);
    fn notify_all(atomic: &AtomicU32);
}

pub struct Spin;

impl RelaxStrategy for Spin {
    // For spinning, we can neither wake nor put to sleep.
    // Just hint that we'll do nothing, and cross our fingers

    fn relax(_atomic: &AtomicU32, _expected: u32) {
        core::hint::spin_loop();
    }

    fn notify_one(atomic: &AtomicU32) {}

    fn notify_all(atomic: &AtomicU32) {}
}

// TODO: when threading is properly set up, a parking strategy
