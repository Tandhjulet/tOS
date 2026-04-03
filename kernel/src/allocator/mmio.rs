use core::sync::atomic::AtomicU64;

use x86_64::{
    PhysAddr, VirtAddr,
    structures::paging::{
        FrameAllocator, Mapper, Page, PageTableFlags, PhysFrame, Size4KiB, mapper::MapToError,
    },
};

use crate::allocator::{FRAME_ALLOCATOR, MAPPER};

pub static PAGE_SIZE: u64 = 0x1000;
pub static NEXT_PHYS: AtomicU64 = AtomicU64::new(0x1000_0000);
pub static NEXT_MMIO: AtomicU64 = AtomicU64::new(0xFFFF_8000_0000_0000);

pub fn alloc_dma_region(size: u64) -> (VirtAddr, PhysAddr) {
    let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let pages = aligned_size / PAGE_SIZE;

    // TODO: fix the BootInfoFrameAllocator to use a Buddy allocator
    // so it's guaranteed the pages are contigous
    let mut frame_guard = FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_guard.as_mut().unwrap();

    let first_frame = frame_allocator.allocate_frame().expect("DMA alloc failed");
    let phys = first_frame.start_address();

    for _ in 1..pages {
        frame_allocator.allocate_frame().expect("DMA alloc failed");
    }

    drop(frame_guard);

    let virt = map_mmio(phys, aligned_size);
    (virt, phys)
}

pub fn map_mmio(phys_addr: PhysAddr, size: u64) -> VirtAddr {
    let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let virt_start =
        VirtAddr::new(NEXT_MMIO.fetch_add(aligned_size, core::sync::atomic::Ordering::SeqCst));

    let phys_start = phys_addr.align_down(PAGE_SIZE);

    let res = map_mmio_region(phys_start, virt_start, aligned_size);
    if let Err(e) = res {
        panic!("Failed to map MMIO region: {:?}", e);
    }

    // if the phys_addr was not page aligned, we need to
    // add the offset to the virt start
    let offset = phys_addr.as_u64() & (PAGE_SIZE - 1);
    virt_start + offset
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

    let mut guard = FRAME_ALLOCATOR.lock();
    let allocator = guard.as_mut().unwrap();

    for page in page_range {
        let frame = PhysFrame::containing_address(PhysAddr::new(
            phys_addr.as_u64() + (page.start_address().as_u64() - virt_start.as_u64()),
        ));

        unsafe { mapper.map_to(page, frame, flags, allocator)?.flush() };
    }

    Ok(())
}
