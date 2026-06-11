use core::ops::{Add, Sub};

use crate::core::align::{align_down, align_up};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
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
        Self(align_down(self.0 as usize, align as usize) as u64)
    }

    pub fn align_up(&self, align: u64) -> Self {
        Self(align_up(self.0 as usize, align as usize) as u64)
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
