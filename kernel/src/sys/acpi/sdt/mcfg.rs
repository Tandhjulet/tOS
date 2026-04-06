use alloc::slice;

use crate::sys::acpi::{AcpiTable, Signature, sdt::SdtHeader};

#[repr(C, packed)]
pub struct Mcfg {
    pub header: SdtHeader,
    _reserved: u64,
    // and n of format McfgEntry
}

unsafe impl AcpiTable for Mcfg {
    const SIGNATURE: Signature = Signature::MCFG;

    fn header(&self) -> &SdtHeader {
        &self.header
    }
}

impl Mcfg {
    pub fn entries(&self) -> &[McfgEntry] {
        let len = self.header.length as usize - size_of::<Mcfg>();
        let num_entries = len / size_of::<McfgEntry>();

        unsafe {
            let ptr = (self as *const Mcfg as *const u8).add(size_of::<Mcfg>()) as *const McfgEntry;
            slice::from_raw_parts(ptr, num_entries)
        }
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct McfgEntry {
    pub base_addr: u64,
    pub seg_grp: u16,
    pub bus_num_start: u8,
    pub bus_num_end: u8,
    _reserved: u32,
}
