use core::ops::{Add, Sub};

use crate::{
    sys::mem::vmm::paging::PageTableIndex,
    util::align::{align_down, align_up},
};

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

    pub const fn align_down(&self, align: u64) -> Self {
        Self(align_down(self.0 as usize, align as usize) as u64)
    }

    pub const fn align_up(&self, align: u64) -> Self {
        Self(align_up(self.0 as usize, align as usize) as u64)
    }

    pub fn is_aligned<U>(&self, align: U) -> bool
    where
        U: Into<u64>,
    {
        self.is_aligned_64(align.into())
    }

    pub fn is_aligned_64(&self, align: u64) -> bool {
        self.align_down(align).0 == self.0
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

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct VirtAddr(u64);

impl VirtAddr {
    pub const fn zero() -> Self {
        Self(0u64)
    }

    pub const fn new(addr: u64) -> Self {
        Self(addr)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }

    pub const fn as_ptr<T>(self) -> *const T {
        self.as_u64() as *const T
    }

    pub const fn as_mut_ptr<T>(self) -> *mut T {
        self.as_ptr::<T>() as *mut T
    }

    pub const fn align_down(&self, align: u64) -> Self {
        Self(align_down(self.0 as usize, align as usize) as u64)
    }

    pub const fn align_up(&self, align: u64) -> Self {
        Self(align_up(self.0 as usize, align as usize) as u64)
    }

    pub fn is_aligned<U>(&self, align: U) -> bool
    where
        U: Into<u64>,
    {
        self.is_aligned_64(align.into())
    }

    pub fn is_aligned_64(&self, align: u64) -> bool {
        self.align_down(align).0 == self.0
    }

    pub(super) const fn pml4(self) -> PageTableIndex {
        PageTableIndex::new_truncated((self.0 >> 12 >> 9 >> 9 >> 9) as u16)
    }

    pub(super) const fn pdpt(self) -> PageTableIndex {
        PageTableIndex::new_truncated((self.0 >> 12 >> 9 >> 9) as u16)
    }

    pub(super) const fn pd(self) -> PageTableIndex {
        PageTableIndex::new_truncated((self.0 >> 12 >> 9) as u16)
    }

    pub(super) const fn pt(self) -> PageTableIndex {
        PageTableIndex::new_truncated((self.0 >> 12) as u16)
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
