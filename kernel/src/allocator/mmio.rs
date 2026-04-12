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

pub fn alloc_dma_region(size: u64) -> MappedRegion {
    let aligned_size = (size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let pages = aligned_size / PAGE_SIZE;

    // TODO: fix the BootInfoFrameAllocator to use a Buddy allocator
    // so it's guaranteed the pages are contigous
    let mut frame_guard = FRAME_ALLOCATOR.lock();
    let frame_allocator = frame_guard.as_mut().unwrap();

    let first_frame = frame_allocator.allocate_frame().expect("DMA alloc failed");
    let phys = first_frame.start_address();
    assert!(phys.is_aligned(PAGE_SIZE), "DMA frame not page-aligned!");

    for _ in 1..pages {
        frame_allocator.allocate_frame().expect("DMA alloc failed");
    }

    drop(frame_guard);

    let region = map_mmio(phys, aligned_size);
    unsafe {
        core::ptr::write_bytes(region.as_mut_ptr::<u8>(), 0, region.len() as usize);
    }

    region
}

pub fn map_mmio(phys_addr: PhysAddr, size: u64) -> MappedRegion {
    let phys_start = phys_addr.align_down(PAGE_SIZE);
    let offset = phys_addr.as_u64() - phys_start.as_u64();

    // align size to account for offset too
    let aligned_size = (size + offset + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let virt_start =
        VirtAddr::new(NEXT_MMIO.fetch_add(aligned_size, core::sync::atomic::Ordering::SeqCst));

    let res = map_mmio_region(phys_start, virt_start, aligned_size);
    if let Err(e) = res {
        panic!("Failed to map MMIO region: {:?}", e);
    }

    MappedRegion {
        virt: virt_start + offset,
        virt_mapped: virt_start,
        phys: phys_start,
        len: aligned_size,
    }
}

fn map_mmio_region(
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

fn unmap_mmio_region(virt_start: VirtAddr, size: u64) {
    let mut guard = MAPPER.lock();
    let mapper = guard.as_mut().unwrap();

    let page_range = {
        let start_page = Page::<Size4KiB>::containing_address(virt_start);
        let end_page = Page::<Size4KiB>::containing_address(virt_start + size - 1u64);

        Page::range_inclusive(start_page, end_page)
    };

    for page in page_range {
        if let Ok((_, flush)) = mapper.unmap(page) {
            flush.flush();
        }
    }
}

#[derive(Debug)]
pub struct MappedRegion {
    virt: VirtAddr,
    phys: PhysAddr,
    virt_mapped: VirtAddr,

    len: u64,
}

impl MappedRegion {
    pub fn phys(&self) -> PhysAddr {
        self.phys
    }

    pub fn virt(&self) -> VirtAddr {
        self.virt
    }

    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn as_ptr<T>(&self) -> *const T {
        self.virt.as_ptr()
    }

    pub fn as_mut_ptr<T>(&self) -> *mut T {
        self.virt.as_mut_ptr()
    }
}

impl Drop for MappedRegion {
    fn drop(&mut self) {
        unmap_mmio_region(self.virt_mapped, self.len);
    }
}
