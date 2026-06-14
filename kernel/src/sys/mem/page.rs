pub trait PageSize: Copy + Eq + PartialOrd + Ord {
    const SIZE: u64;
    const LABEL: &'static str;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Size4KiB;

impl PageSize for Size4KiB {
    const SIZE: u64 = 4096;
    const LABEL: &'static str = "4KiB";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Size2MiB;

impl PageSize for Size2MiB {
    const SIZE: u64 = Size4KiB::SIZE * 512;
    const LABEL: &'static str = "2MiB";
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Size1GiB;

impl PageSize for Size1GiB {
    const SIZE: u64 = Size2MiB::SIZE * 512;
    const LABEL: &'static str = "1GiB";
}

#[derive(Debug)]
pub struct AddressNotAligned;
