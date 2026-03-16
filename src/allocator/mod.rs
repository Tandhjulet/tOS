pub mod fixed_size_block;
pub mod memory;

use bootloader::BootInfo;
use spin::Mutex;
use x86_64::{
    PhysAddr, VirtAddr,
    structures::paging::{
        FrameAllocator, Mapper, OffsetPageTable, Page, PageTableFlags, PhysFrame, Size4KiB,
        mapper::MapToError,
    },
};

use crate::allocator::{
    fixed_size_block::FixedSizeBlockAllocator,
    memory::{BootInfoFrameAllocator, EmptyFrameAllocator},
};

pub const HEAP_START: usize = 0x_4444_4444_0000;
pub const HEAP_SIZE: usize = 100 * 1024; // 100 KiB

static MAPPER: Mutex<Option<OffsetPageTable<'static>>> = Mutex::new(None);

pub fn init(boot_info: &'static BootInfo) -> Result<(), MapToError<Size4KiB>> {
    let phys_mem_offset = VirtAddr::new(boot_info.physical_memory_offset);
    let mapper = unsafe { memory::init(phys_mem_offset) };
    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(&boot_info.memory_map) };
    *MAPPER.lock() = Some(mapper);

    init_heap(&mut frame_allocator)
}

pub fn init_heap(
    frame_allocator: &mut impl FrameAllocator<Size4KiB>,
) -> Result<(), MapToError<Size4KiB>> {
    let page_range = {
        let heap_start = VirtAddr::new(HEAP_START as u64);
        let heap_end = heap_start + HEAP_SIZE - 1u64;
        let heap_start_page = Page::containing_address(heap_start);
        let heap_end_page = Page::containing_address(heap_end);
        Page::range_inclusive(heap_start_page, heap_end_page)
    };

    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().unwrap();

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

pub fn map_mmio_region(
    phys_addr: PhysAddr,
    virt_start: VirtAddr,
    size: u64,
) -> Result<(), MapToError<Size4KiB>> {
    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().unwrap();

    let page_range = {
        let start_page = Page::<Size4KiB>::containing_address(virt_start);
        let end_page = Page::<Size4KiB>::containing_address(virt_start + size - 1u64);

        Page::range_inclusive(start_page, end_page)
    };

    let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE | PageTableFlags::NO_CACHE;

    for page in page_range {
        let frame = PhysFrame::containing_address(PhysAddr::new(
            phys_addr.as_u64() + (page.start_address().as_u64() - virt_start.as_u64()),
        ));

        unsafe {
            mapper
                .map_to(page, frame, flags, &mut EmptyFrameAllocator)?
                .flush()
        };
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

    pub fn lock(&self) -> spin::MutexGuard<A> {
        self.inner.lock()
    }
}

#[global_allocator]
static ALLOCATOR: Locked<FixedSizeBlockAllocator> = Locked::new(FixedSizeBlockAllocator::new());
