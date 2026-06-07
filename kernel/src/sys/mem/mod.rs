use crate::sys::mem::page::{PageSize, Size4KiB};

pub const FRAME_SIZE: u64 = Size4KiB::SIZE;

pub mod addr;
pub mod frame;
pub mod page;
pub mod pmm;
