use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Deref, DerefMut, Index, IndexMut};

use crate::sys::mem::{addr::PhysAddr, frame::PhysFrame, vmm::mapper::TranslateError};

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
#[repr(transparent)]
pub struct PageTableFlags(u64);

// Refer to https://wiki.osdev.org/images/6/60/Page_table_entry.png

impl PageTableFlags {
    pub const EMPTY: Self = Self(0);
    pub const PRESENT: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const USER: Self = Self(1 << 2);
    pub const WRITE_THROUGH: Self = Self(1 << 3);
    pub const CACHE_DISABLED: Self = Self(1 << 4);
    pub const ACCESSED: Self = Self(1 << 5);
    pub const DIRTY: Self = Self(1 << 6);
    pub const HUGE_PAGE: Self = Self(1 << 7);
    pub const GLOBAL: Self = Self(1 << 8);
    pub const NO_EXECUTE: Self = Self(1 << 63);

    #[inline]
    pub const fn bits(self) -> u64 {
        self.0
    }

    pub const fn empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == self.0
    }

    pub fn set(&mut self, flag: Self) {
        self.0 |= flag.0;
    }

    pub fn unset(&mut self, flag: Self) {
        self.0 &= !flag.0;
    }
}

impl BitOr for PageTableFlags {
    type Output = PageTableFlags;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitAnd for PageTableFlags {
    type Output = PageTableFlags;

    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitOrAssign for PageTableFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAndAssign for PageTableFlags {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

// Bits [12-51) hold the phys frame num
const PHYS_MASK: u64 = 0x000f_ffff_ffff_f000;

#[derive(Clone, Copy, Debug)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    #[inline]
    pub const fn unused() -> Self {
        Self(0)
    }

    #[inline]
    pub fn is_unused(&self) -> bool {
        self.0 == 0
    }

    #[inline]
    pub fn clear(&mut self) {
        self.0 = 0;
    }

    #[inline]
    pub fn is_present(&self) -> bool {
        self.flags().contains(PageTableFlags::PRESENT)
    }

    #[inline]
    pub fn is_huge(&self) -> bool {
        self.flags().contains(PageTableFlags::HUGE_PAGE)
    }

    #[inline]
    pub fn flags(&self) -> PageTableFlags {
        PageTableFlags(self.0 & !PHYS_MASK)
    }

    #[inline]
    pub fn addr(&self) -> PhysAddr {
        PhysAddr::new(self.0 & PHYS_MASK)
    }

    #[inline]
    pub fn frame(self) -> Result<PhysFrame, FrameError> {
        if !self.is_present() {
            Err(FrameError::FrameNotPresent)
        } else if self.is_huge() {
            Err(FrameError::HugeFrame)
        } else {
            Ok(PhysFrame::containing_address(self.addr()))
        }
    }

    #[inline]
    pub fn set_addr(&mut self, addr: PhysAddr) {
        self.0 = (self.0 & !PHYS_MASK) | (addr.as_u64() & PHYS_MASK);
    }

    #[inline]
    pub fn set_flags(&mut self, flags: PageTableFlags) {
        self.0 = self.addr().as_u64() | flags.bits();
    }

    #[inline]
    pub fn set_addr_and_flags(&mut self, addr: PhysAddr, flags: PageTableFlags) {
        self.0 = (addr.as_u64() & PHYS_MASK) | flags.bits();
    }

    #[inline]
    pub fn set_frame(&mut self, frame: PhysFrame, flags: PageTableFlags) {
        assert!(!flags.contains(PageTableFlags::HUGE_PAGE));
        self.set_addr_and_flags(frame.start_addr(), flags);
    }
}

pub const TABLE_ENTRY_COUNT: usize = 512;

#[repr(C, align(4096))]
pub struct PageTable {
    entries: [PageTableEntry; TABLE_ENTRY_COUNT],
}

impl PageTable {
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry::unused(); TABLE_ENTRY_COUNT],
        }
    }

    #[inline]
    pub fn zero(&mut self) {
        for entry in self.entries.iter_mut() {
            entry.clear();
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.iter().all(|entry| entry.is_unused())
    }
}

impl Deref for PageTable {
    type Target = [PageTableEntry; TABLE_ENTRY_COUNT];

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl DerefMut for PageTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FrameError {
    FrameNotPresent,
    HugeFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PageTableIndex(u16);

impl PageTableIndex {
    pub const fn new(value: u16) -> Self {
        assert!((value as usize) < TABLE_ENTRY_COUNT);
        Self(value)
    }

    pub const fn new_truncated(value: u16) -> Self {
        Self(value & 0x1ff)
    }
}

impl Index<PageTableIndex> for PageTable {
    type Output = PageTableEntry;

    fn index(&self, index: PageTableIndex) -> &Self::Output {
        &self.entries[usize::from(index)]
    }
}

impl IndexMut<PageTableIndex> for PageTable {
    fn index_mut(&mut self, index: PageTableIndex) -> &mut Self::Output {
        &mut self.entries[usize::from(index)]
    }
}

impl From<PageTableIndex> for usize {
    #[inline]
    fn from(index: PageTableIndex) -> Self {
        usize::from(index.0)
    }
}
