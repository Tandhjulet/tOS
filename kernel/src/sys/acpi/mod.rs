use core::{iter, marker::PhantomData, pin::Pin};

use alloc::{borrow::ToOwned, string::String};
use bootloader_api::info::Optional;
use conquer_once::spin::OnceCell;
use x86_64::PhysAddr;

use crate::{
    allocator::mmio::{self, MappedRegion},
    sys::acpi::{rsdp::Rsdp, sdt::SdtHeader},
};

pub mod rsdp;
pub mod sdt;

pub static ACPI: OnceCell<Acpi> = OnceCell::uninit();

/**
 * This ACPI implementation is heavily inspired by https://github.com/rust-osdev/acpi
 * and the amazing documentation at wiki.osdev.org!
 */
pub struct Acpi {
    tables: AcpiTables,
}

impl Acpi {
    pub fn try_init(rsdp: Optional<u64>) -> Result<(), String> {
        if let Some(&rsdp) = rsdp.as_ref() {
            Self::init(rsdp)
        } else {
            Err("RSDP not provided".to_owned())
        }
    }

    pub fn init(rsdp: u64) -> Result<(), String> {
        if ACPI.is_initialized() {
            return Err("ACPI already initialized!".to_owned());
        }

        let tables = unsafe { AcpiTables::from_rsdp(rsdp) }?;
        ACPI.init_once(|| Self { tables });

        Ok(())
    }

    pub fn tables(&self) -> &AcpiTables {
        &self.tables
    }
}

pub struct AcpiTables {
    rsdt_region: MappedRegion,
    pub revision: u8,
}

impl AcpiTables {
    pub unsafe fn from_rsdp(raw_rsdp_addr: u64) -> Result<Self, String> {
        let rsdp_addr = PhysAddr::new(raw_rsdp_addr);
        let rsdp_region = mmio::map_mmio(rsdp_addr, size_of::<Rsdp>() as u64);

        let rsdp = unsafe { *rsdp_region.as_ptr::<Rsdp>() };
        rsdp.validate()?;

        let revision = rsdp.revision;
        let addr = rsdp.address();

        unsafe { Self::from_rsdt(revision, addr) }
    }

    pub unsafe fn from_rsdt(revision: u8, raw_rsdt_addr: u64) -> Result<Self, String> {
        let rsdt_addr = PhysAddr::new(raw_rsdt_addr);
        let rsdt_region = mmio::map_mmio(rsdt_addr, size_of::<SdtHeader>() as u64);
        let rsdt = unsafe { *rsdt_region.as_ptr::<SdtHeader>() };

        let rsdt_region = mmio::map_mmio(rsdt_addr, rsdt.length as u64);

        Ok(Self {
            rsdt_region,
            revision,
        })
    }

    pub fn rsdt(&self) -> &SdtHeader {
        unsafe { &*self.rsdt_region.as_ptr::<SdtHeader>() }
    }

    pub fn table_entries(&self) -> impl Iterator<Item = usize> {
        let entry_size: usize = if self.revision == 0 { 4 } else { 8 };
        let header_start = self.rsdt_region.virt().as_ptr::<u8>();
        let mut entries_ptr = unsafe { header_start.byte_add(size_of::<SdtHeader>()) };

        let mut num_entries = (self.rsdt().length as usize - size_of::<SdtHeader>()) / entry_size;

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

    pub fn find_tables<T: AcpiTable>(&self) -> impl Iterator<Item = MappedTable<T>> {
        self.table_entries().filter_map(|raw_phys_addr| {
            let phys_addr = PhysAddr::new(raw_phys_addr as u64);

            let header_region = mmio::map_mmio(phys_addr, size_of::<SdtHeader>() as u64);
            let header = unsafe { &*header_region.as_ptr::<SdtHeader>() };

            if header.signature == T::SIGNATURE {
                let len = header.length;

                Some(MappedTable {
                    region: mmio::map_mmio(phys_addr, len as u64),
                    _phantom: PhantomData,
                })
            } else {
                None
            }
        })
    }

    pub fn find_table<T: AcpiTable>(&self) -> Option<MappedTable<T>> {
        self.find_tables().next()
    }
}

pub unsafe trait AcpiTable {
    const SIGNATURE: Signature;

    fn header(&self) -> &SdtHeader;
}

pub struct MappedTable<T: AcpiTable> {
    region: MappedRegion,
    _phantom: PhantomData<T>,
}

impl<T: AcpiTable> MappedTable<T> {
    pub fn get(&self) -> Pin<&T> {
        unsafe { Pin::new_unchecked(self.as_ref()) }
    }

    pub unsafe fn as_ref(&self) -> &T {
        unsafe { &*self.region.as_ptr() }
    }

    pub unsafe fn as_mut_ref(&self) -> &mut T {
        unsafe { &mut *self.region.as_mut_ptr() }
    }

    pub fn as_ptr(&self) -> *const T {
        self.region.as_ptr()
    }

    pub fn as_mut_ptr(&self) -> *mut T {
        self.region.as_mut_ptr()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct Signature([u8; 4]);

impl Signature {
    pub const MCFG: Signature = Signature(*b"MCFG");
    pub const FADT: Signature = Signature(*b"FADT");
    pub const MADT: Signature = Signature(*b"APIC");
}

#[repr(C, packed)]
pub struct GenericAddress {
    pub address_space: u8,
    pub bit_width: u8,
    pub bit_offset: u8,
    pub access_size: u8,
    pub address: u64,
}
