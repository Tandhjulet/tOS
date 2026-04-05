use alloc::string::String;
use x86_64::PhysAddr;

use crate::{allocator::mmio, sys::acpi::rsdp::Rsdp};

pub mod rsdp;

#[derive(Clone)]
pub struct Acpi;

impl Acpi {
    pub fn init(rsdp: u64) -> Result<(), String> {
        let tables = unsafe { AcpiTables::from_rsdp(rsdp) }?;

        Ok(())
    }
}

pub struct AcpiTables {}

impl AcpiTables {
    pub unsafe fn from_rsdp(raw_rsdp_addr: u64) -> Result<(), String> {
        let rsdp_addr = PhysAddr::new(raw_rsdp_addr);
        let rsdp_virt = mmio::map_mmio(rsdp_addr, size_of::<Rsdp>() as u64);

        let rsdp = unsafe { *rsdp_virt.as_ptr::<Rsdp>() };
        rsdp.validate()?;

        let revision = rsdp.revision;
        let addr = rsdp.address();

        Ok(())
    }

    pub unsafe fn from_rsdt(revision: u8, addr: usize) {}
}
