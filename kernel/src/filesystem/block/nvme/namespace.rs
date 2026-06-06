use alloc::sync::Arc;
use spin::Mutex;

use crate::{
    allocator::mmio::alloc_dma_region,
    filesystem::block::{
        BlockDevice, BlockDeviceError, BlockDeviceIo,
        nvme::{
            commands::ReadCommand,
            queue::{Io, QueuePair},
        },
    },
};

pub struct NvmeNamespace {
    pub queue: Arc<Mutex<QueuePair<Io>>>,
    pub nsid: u32,
    pub command_set: NvmeCommandSet,

    // CNS 0x08 - command-set-indepedent fields
    pub independent: IdentifyNamespaceIndependent,
}

impl BlockDevice for NvmeNamespace {
    fn block_size(&self) -> u32 {
        self.command_set.block_size()
    }

    fn block_count(&self) -> u64 {
        self.command_set.block_count()
    }
}

impl BlockDeviceIo for NvmeNamespace {
    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        let block_size = self.block_size() as u64;
        let count = buf.len() as u64 / block_size;

        if lba + count > self.block_count() {
            return Err(BlockDeviceError::InvalidRange { lba, count });
        }

        if buf.len() as u64 % block_size != 0 {
            return Err(BlockDeviceError::NotAligned);
        }

        let dma = alloc_dma_region(count * block_size);

        let future = {
            let mut queue = self.queue.lock();
            queue.submit(ReadCommand {
                prp: dma.phys().as_u64(),
                slba: lba,
                cdw12: count as u32,
                dsm: 0,
            })
        };

        let entry = future.await;
        if !entry.status.is_success() {
            return Err(BlockDeviceError::IoError);
        }

        let src = unsafe { core::slice::from_raw_parts(dma.virt().as_ptr(), buf.len()) };
        buf.copy_from_slice(src);

        Ok(())
    }

    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        todo!()
    }

    async fn flush(&mut self) -> Result<(), BlockDeviceError> {
        todo!()
    }
}

impl NvmeCommandSet {
    pub fn block_size(&self) -> u32 {
        match self {
            NvmeCommandSet::Nvm(data) => data.identify.block_size(),
            _ => todo!(),
        }
    }

    pub fn block_count(&self) -> u64 {
        match self {
            NvmeCommandSet::Nvm(data) => data.identify.block_count(),
            _ => todo!(),
        }
    }
}

pub enum NvmeCommandSet {
    Nvm(NvmNamespaceData),
    KeyValue(),
    Zoned(),
    Computational(),
    SubsystemLocalMemory(),
}

pub struct NvmNamespaceData {
    pub identify: IdentifyNamespaceNvm,
    pub specific: IdentifyNamespaceSpecificNvm,
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

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyNamespaceSpecificNvm {
    pub _todo: [u8; 4096],
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
