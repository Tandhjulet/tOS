use crate::sys::mem::page_size::{PageSize, Size4KiB};

pub const FRAME_SIZE: u64 = Size4KiB::SIZE;

pub mod addr;
pub mod dma;
pub mod frame;
pub mod heap;
pub mod mmio;
pub mod page_size;
pub mod pmm;
pub mod vmm;
