use bootloader_api::BootInfo;

use crate::sys::mem::{
    addr::VirtAddr,
    page::{PageSize, Size4KiB},
    pmm::boot::BootFrameAllocator,
    vmm::{mapper::mapped_page_table::MappedPageTable, paging::PageTable},
};

pub const FRAME_SIZE: u64 = Size4KiB::SIZE;

pub mod addr;
pub mod dma;
pub mod frame;
pub mod heap;
pub mod mmio;
pub mod page;
pub mod pmm;
pub mod vmm;

pub fn init(boot_info: &'static BootInfo) {
    let (offset, regions) = { (boot_info.physical_memory_offset, &boot_info.memory_regions) };

    let allocator = unsafe { BootFrameAllocator::init(regions) };

    let offset = VirtAddr::new(*offset.as_ref().unwrap());
    let level_4_table = unsafe { active_level_4_table(offset) };
    let mapper = MappedPageTable::new(level_4_table, offset);
}

unsafe fn active_level_4_table(phys_mem_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();

    let phys = level_4_table_frame.start_address();
    let virt = phys_mem_offset + phys.as_u64();
    let ptr: *mut PageTable = virt.as_mut_ptr();

    unsafe { &mut *ptr }
}
