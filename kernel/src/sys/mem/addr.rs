use core::ops::{Add, Sub};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysAddr(u64);

impl PhysAddr {
    pub const fn zero() -> Self {
        Self(0u64)
    }

    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn align_down(&self, align: u64) -> Self {
        assert!(align.is_power_of_two(), "`align` must be a power of two");
        Self(self.0 & !(align - 1))
    }

    pub fn align_up(&self, align: u64) -> Self {
        assert!(align.is_power_of_two(), "`align` must be a power of two");
        Self((self.0 + align - 1) & !(align - 1))
    }
}

impl Add<u64> for PhysAddr {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl Sub<u64> for PhysAddr {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0 - rhs)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtAddr(u64);

impl VirtAddr {
    pub const fn zero() -> Self {
        Self(0u64)
    }

    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl Add<u64> for VirtAddr {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Self(self.0 + rhs)
    }
}

impl Sub<u64> for VirtAddr {
    type Output = Self;

    fn sub(self, rhs: u64) -> Self::Output {
        Self(self.0 - rhs)
    }
}
