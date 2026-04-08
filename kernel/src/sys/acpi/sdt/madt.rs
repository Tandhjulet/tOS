use core::{
    marker::{PhantomData, PhantomPinned},
    pin::Pin,
};

use num_enum::TryFromPrimitive;

use crate::{println, sys::acpi::sdt::SdtHeader};

#[repr(C, packed)]
pub struct Madt {
    pub header: SdtHeader,

    pub apic_addr: u32,
    pub flags: u32,

    _pinned: PhantomPinned,
}

impl<'a> Madt {
    pub fn entries(self: Pin<&Self>) -> MadtEntryIter<'a> {
        const MADT_SIZE: usize = size_of::<Madt>();

        let ptr = unsafe { Pin::into_inner_unchecked(self) } as *const _ as *const u8;

        MadtEntryIter {
            pointer: unsafe { ptr.add(MADT_SIZE) },
            remaining_length: self.header.length - (MADT_SIZE as u32),
            _phantom: PhantomData,
        }
    }
}

pub struct MadtEntryIter<'a> {
    pointer: *const u8,
    remaining_length: u32,

    _phantom: PhantomData<&'a ()>,
}

/**
 * See UEFI documentation at
 * https://uefi.org/specs/ACPI/6.5/05_ACPI_Software_Programming_Model.html#multiple-apic-description-table-madt
 */
#[derive(TryFromPrimitive, Clone, Copy, Debug)]
#[repr(u8)]
pub enum MadtEntryKind {
    LocalApic = 0,
    IoApic = 1,
    InterruptSourceOverride = 2,
    NmiSource = 3,
    LocalApicNmi = 4,
    LocalApicAddressOverride = 5,
    IoSapic = 6,
    LocalSapic = 7,
    PlatformInterruptSources = 8,
    Local2Apic = 9,
    Local2ApicNvmi = 0xA,
    GicCpu = 0xB,
    GicDist = 0xC,
    GicMsi = 0xD,
    GicRedist = 0xE,
    GicIts = 0xF,
    MultiprocessorWakeup = 0x10,
    CorePic = 0x11,
    LioPic = 0x12,
    HtPic = 0x13,
    EioPic = 0x14,
    MsiPic = 0x15,
    BioPic = 0x16,
    LpcPic = 0x17,
}

impl<'a> MadtEntryKind {
    pub unsafe fn to_entry(&self, ptr: *const u8) -> Option<MadtEntry<'a>> {
        let entry = match self {
            MadtEntryKind::LocalApic => {
                MadtEntry::LocalApic(unsafe { &*(ptr as *const LocalApicEntry) })
            }
            MadtEntryKind::IoApic => MadtEntry::IoApic(unsafe { &*(ptr as *const IoApicEntry) }),
            kind => {
                println!(
                    "Skipping converting unimplemented kind {:?} to MadtEntry",
                    kind
                );
                return None;
            }
        };

        Some(entry)
    }
}

#[derive(Debug)]
pub enum MadtEntry<'a> {
    LocalApic(&'a LocalApicEntry),
    IoApic(&'a IoApicEntry),
    InterruptSourceOverride,
    NmiSource,
    LocalApicNmi,
    LocalApicAddressOverride,
    Local2Apic,
    Local2ApicNmi,
    GicCpu,
    GicDist,
    GicMsi,
    GicRedist,
    GicIts,
    MultiprocessorWakeup,
    CorePic,
    LioPic,
    HtPic,
    EioPic,
    MsiPic,
    BioPic,
    LpcPic,
}

#[derive(Clone, Copy, Debug)]
#[repr(C, packed)]
pub struct EntryHeader {
    pub entry_type: u8,
    pub length: u8,
}

impl<'a> Iterator for MadtEntryIter<'a> {
    type Item = MadtEntry<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.remaining_length > 0 {
            let header = unsafe { *(self.pointer as *const EntryHeader) };
            if self.remaining_length < header.length as u32 {
                return None;
            }

            let entry_ptr = self.pointer;
            self.pointer = unsafe { self.pointer.byte_add(header.length as usize) };
            self.remaining_length -= header.length as u32;

            let Ok(kind) = MadtEntryKind::try_from(header.entry_type) else {
                continue;
            };

            let entry = unsafe { kind.to_entry(entry_ptr) };
            if entry.is_none() {
                continue;
            }

            return entry;
        }

        None
    }
}

#[derive(Debug)]
pub struct LocalApicEntry {
    pub header: EntryHeader,
    pub acpi_processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
}

#[derive(Debug)]
pub struct IoApicEntry {
    pub header: EntryHeader,
    pub io_apic_id: u8,
    _reserved: u8,
    pub io_apic_addr: u64,
    pub gsi_base: u64,
}
