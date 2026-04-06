use core::iter;

use alloc::string::String;
use x86_64::PhysAddr;

use crate::{
    allocator::mmio,
    sys::acpi::{rsdp::Rsdp, sdt::SdtHeader},
};

pub mod rsdp;
pub mod sdt;

/**
 * This ACPI implementation is heavily inspired by https://github.com/rust-osdev/acpi
 * and the amazing documentation at wiki.osdev.org!
 */
#[derive(Clone)]
pub struct Acpi;

impl Acpi {
    pub fn init(rsdp: u64) -> Result<(), String> {
        let tables = unsafe { AcpiTables::from_rsdp(rsdp) }?;

        Ok(())
    }
}

pub struct AcpiTables {
    rsdt: SdtHeader,
    pub revision: u8,
}

impl AcpiTables {
    pub unsafe fn from_rsdp(raw_rsdp_addr: u64) -> Result<Self, String> {
        let rsdp_addr = PhysAddr::new(raw_rsdp_addr);
        let rsdp_virt = mmio::map_mmio(rsdp_addr, size_of::<Rsdp>() as u64);

        let rsdp = unsafe { *rsdp_virt.as_ptr::<Rsdp>() };
        rsdp.validate()?;

        let revision = rsdp.revision;
        let addr = rsdp.address();

        unsafe { Self::from_rsdt(revision, addr) }
    }

    pub unsafe fn from_rsdt(revision: u8, raw_rsdt_addr: u64) -> Result<Self, String> {
        let rsdt_addr = PhysAddr::new(raw_rsdt_addr);
        let rsdt_virt = mmio::map_mmio(rsdt_addr, size_of::<SdtHeader>() as u64);
        let rsdt = unsafe { *rsdt_virt.as_ptr::<SdtHeader>() };

        let rsdt_virt = mmio::map_mmio(rsdt_addr, rsdt.length as u64);
        let rsdt = unsafe { *rsdt_virt.as_ptr::<SdtHeader>() };

        Ok(Self { rsdt, revision })
    }

    pub fn table_entries(&self) -> impl Iterator<Item = usize> {
        let entry_size: usize = if self.revision == 0 { 4 } else { 8 };
        let header_start = (&self.rsdt) as *const SdtHeader;
        let mut entries_ptr = unsafe { header_start.byte_add(size_of::<SdtHeader>()).cast::<u8>() };

        let mut num_entries = (self.rsdt.length as usize - size_of::<SdtHeader>()) / entry_size;

        iter::from_fn(move || {
            if num_entries > 0 {
                let entry = unsafe {
                    let entry = if self.revision == 0 {
                        *entries_ptr.cast::<u32>() as usize
                    } else {
                        *entries_ptr.cast::<u64>() as usize
                    };
                    entries_ptr = entries_ptr.byte_add(entry_size);
                    entry
                };

                num_entries -= 1;
                Some(entry)
            } else {
                None
            }
        })
    }

    pub fn find_tables<T: AcpiTable>(&self) -> impl Iterator<Item = *mut T> {
        self.table_entries().filter_map(|raw_phys_addr| {
            let phys_addr = PhysAddr::new(raw_phys_addr as u64);

            // TODO: unmap virt
            let header_virt = mmio::map_mmio(phys_addr, size_of::<SdtHeader>() as u64);
            let header = unsafe { &*header_virt.as_ptr::<SdtHeader>() };
            if header.signature == T::SIGNATURE {
                let len = header.length;

                let table_virt = mmio::map_mmio(phys_addr, len as u64);
                Some(table_virt.as_mut_ptr::<T>())
            } else {
                None
            }
        })
    }
}

pub unsafe trait AcpiTable {
    const SIGNATURE: Signature;

    fn header(&self) -> &SdtHeader;
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Signature([u8; 4]);

impl Signature {
    pub const MCFG: Signature = Signature(*b"MCFG");
}
