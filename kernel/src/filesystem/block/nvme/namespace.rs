use alloc::string::String;

use crate::filesystem::block::StorageDevice;

pub struct NvmeNamespace {
    pub nsid: u32,
    pub csi: u8,

    // CNS 0x00 - only NVM-based command sets (NVM and Zoned NS)
    // Contains LBA formats, cap, metadata cap
    pub nvm_base: Option<IdentifyNamespaceNvm>,

    // CNS 0x08 - command-set-indepedent fields
    pub independent: IdentifyNamespaceIndependent,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyNamespaceList {
    pub namespaces: [u32; 1024],
}

impl IdentifyNamespaceList {
    pub fn valid(&self) -> impl Iterator<Item = &u32> {
        self.namespaces.iter().take_while(|&&n| n != 0)
    }
}

///
/// Refer to https://nvmexpress.org/wp-content/uploads/NVM-Express-NVM-Command-Set-Specification-Revision-1.1-2024.08.05-Ratified.pdf
/// figure 114 for documentation regarding the implementation
///
#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyNamespaceNvm {
    pub nsze: u64,             // size
    pub ncap: u64,             // capacity
    pub nuse: u64,             // utilization
    pub nsfeat: u8,            // features
    pub nlbaf: u8,             // number of LBA formats
    pub flbas: u8,             // formatted LBA size
    pub _reserved: [u8; 73],   // fields 0x1B - 0x63 are not yet implemented
    pub lbaf: [LbaFormat; 64], // lba format support
    pub _pad: [u8; 3740],
}

impl IdentifyNamespaceNvm {
    pub fn active_lbaf_idx(&self) -> usize {
        let low = (self.flbas & 0xF) as usize;
        let high = (self.flbas >> 5 & 0x3) as usize;
        (high << 4) | low
    }

    pub fn active_lbaf(&self) -> LbaFormat {
        self.lbaf[self.active_lbaf_idx()]
    }

    pub fn block_size(&self) -> u32 {
        1 << self.active_lbaf().lbads
    }

    pub fn block_count(&self) -> u64 {
        self.nsze
    }

    pub fn size_bytes(&self) -> u64 {
        self.nsze * self.block_size() as u64
    }
}

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct LbaFormat {
    pub ms: u16,   // Metadata Size per LBA
    pub lbads: u8, // LBA Data Size (reported as 2^self.lbads)
    pub rp: u8,    // Relative Performance
}

// See Figure 280 in the base specification
#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyNamespaceIndependent {
    pub nsfeat: u8,    // namespace features
    pub nmic: u8,      // multi-path I/O and sharing capabilities
    pub rescap: u8,    // reservation capabilities
    pub fpi: u8,       // format progress indicator
    pub anagrpid: u32, // ANA group identifier
    pub nsattr: u8,    // namespace attributes
    pub _reserved: u8,
    pub nvmsetid: u16, // NVM set identifier
    pub endgid: u16,   // endurance group identifier
    pub nstat: u8,     // namespace status
    pub _reserved2: [u8; 4081],
}

impl StorageDevice for NvmeNamespace {
    type Error = String;

    fn read_blocks(&mut self, lba: u64, count: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        todo!()
    }

    fn write_blocks(&mut self, lba: u64, count: u64, buf: &[u8]) -> Result<(), Self::Error> {
        todo!()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        todo!()
    }
}
