use core::range::Range;

use alloc::{borrow::ToOwned, slice, string::String};

const RSDP_SIGNATURE: [u8; 8] = *b"RSD PTR ";
const RSDP_V1_LEN: usize = 20;

#[derive(Clone, Copy)]
#[repr(C, packed)]
pub struct Rsdp {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_addr: u32, // Deprecated since v. 2.0

    // Only for ACPI v. 2.0+
    pub length: u32,
    pub xsdt_addr: u64,
    pub ext_checksum: u8,
    _reserved: [u8; 3],
}

impl Rsdp {
    pub fn validate(&self) -> Result<(), String> {
        if self.signature != RSDP_SIGNATURE {
            return Err("Detected incorrect RSDP signature".to_owned());
        }

        let length = if self.revision > 0 {
            self.length as usize
        } else {
            RSDP_V1_LEN
        };

        let bytes = unsafe { slice::from_raw_parts(self as *const Rsdp as *const u8, length) };
        let sum = bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte));

        if sum != 0 {
            return Err("Found invalid RSDP checksum".to_owned());
        }

        Ok(())
    }

    pub fn address(&self) -> usize {
        if self.revision == 0 {
            self.rsdt_addr as usize
        } else {
            self.xsdt_addr as usize
        }
    }
}
