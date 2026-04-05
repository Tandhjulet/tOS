use alloc::string::String;
use x86_64::PhysAddr;

use crate::{
    allocator::mmio,
    sys::acpi::{rsdp::Rsdp, sdt::SdtHeader},
};

pub mod rsdp;
pub mod sdt;

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
}
