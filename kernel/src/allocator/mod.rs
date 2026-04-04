pub mod memory;
pub mod mmio;
pub mod paging;

use bootloader_api::BootInfo;
use spin::Mutex;
use x86_64::{
    VirtAddr,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB, mapper::MapToError,
    },
};

use crate::allocator::{memory::BootInfoFrameAllocator, paging::FixedSizeBlockAllocator};

pub const HEAP_START: usize = 0x_4444_4444_0000;
pub const HEAP_SIZE: usize = 10 * 1000 * 1024; // 10 MB

pub static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);
pub static FRAME_ALLOCATOR: Mutex<Option<BootInfoFrameAllocator>> = Mutex::new(None);

pub fn init(boot_info: &'static BootInfo) -> Result<(), MapToError<Size4KiB>> {
    let (offset, regions) = { (boot_info.physical_memory_offset, &boot_info.memory_regions) };

    let phys_mem_offset = VirtAddr::new(*offset.as_ref().unwrap());
    let mapper = unsafe { memory::init(phys_mem_offset) };
    *MAPPER.lock() = Some(mapper);

    let frame_allocator = unsafe { BootInfoFrameAllocator::init(regions) };
    *FRAME_ALLOCATOR.lock() = Some(frame_allocator);

    init_heap()
}

fn init_heap() -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    let mut mapper_guard = MAPPER.lock();
    let mapper = mapper_guard.as_mut().unwrap();

    let mut frame_guard = FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_guard.as_mut().unwrap();

    for page in page_range {
        let frame = frame_allocator
            .allocate_frame()
            .ok_or(MapToError::FrameAllocationFailed)?;

        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe { mapper.map_to(page, frame, flags, frame_allocator)?.flush() };
    }

    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}

pub struct Locked<A> {
    inner: spin::Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: spin::Mutex::new(inner),
        }
    }

    pub fn lock(&self) -> spin::MutexGuard<'_, A> {
        self.inner.lock()
    }
}

#[global_allocator]
pub static ALLOCATOR: Locked<FixedSizeBlockAllocator> = Locked::new(FixedSizeBlockAllocator::new());
