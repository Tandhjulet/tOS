use core::fmt::Display;

use alloc::{collections::btree_map::BTreeMap, string::String, sync::Arc};
use spin::Mutex;

pub mod nvme;

static REGISTRY: Mutex<BlockDeviceRegistry> = Mutex::new(BlockDeviceRegistry::new());

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }
}

impl Display for DeviceId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "/dev/{}", self.0)
    }
}

pub struct BlockDeviceRegistry {
    devices: BTreeMap<DeviceId, Arc<Mutex<dyn BlockDevice>>>,
}

impl BlockDeviceRegistry {
    pub const fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    pub fn register(&mut self, id: DeviceId, device: Arc<Mutex<dyn BlockDevice>>) {
        self.devices.insert(id, device);
    }

    pub fn get(&self, id: &DeviceId) -> Option<Arc<Mutex<dyn BlockDevice>>> {
        self.devices.get(id).cloned()
    }

    pub fn count(&self) -> usize {
        self.devices.len()
    }
}

pub trait BlockDevice: Send + Sync {
    fn block_size(&self) -> u32;
    fn block_count(&self) -> u64;

    fn capacity(&self) -> u64 {
        self.block_size() as u64 * self.block_count()
    }

    fn validate_range(&self, lba: u64, buf_len: usize) -> Result<u64, BlockDeviceError> {
        let block_size = self.block_size() as u64;

        if buf_len as u64 % block_size != 0 {
            return Err(BlockDeviceError::NotAligned);
        }

        let count = buf_len as u64 / block_size;

        if lba + count > self.block_count() {
            return Err(BlockDeviceError::InvalidRange { lba, count });
        }

        Ok(count)
    }
}

pub trait BlockDeviceIo: BlockDevice {
    fn read(
        &mut self,
        lba: u64,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<(), BlockDeviceError>> + Send;
    fn write(
        &mut self,
        lba: u64,
        buf: &[u8],
    ) -> impl Future<Output = Result<(), BlockDeviceError>> + Send;
    fn flush(&mut self) -> impl Future<Output = Result<(), BlockDeviceError>> + Send;
}

pub enum BlockDeviceError {
    IoError,
    InvalidRange { lba: u64, count: u64 },
    NotAligned,
    DeviceFault(String),
}
