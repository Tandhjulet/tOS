use core::fmt::Display;

use alloc::{boxed::Box, collections::btree_map::BTreeMap, string::String, sync::Arc};
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
    devices: BTreeMap<DeviceId, Box<dyn BlockDevice>>,
}

impl BlockDeviceRegistry {
    pub const fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
        }
    }

    pub fn register<T>(
        &mut self,
        id: DeviceId,
        device: Arc<Mutex<T>>,
        block_size: u32,
        block_count: u64,
    ) where
        T: StorageDevice + Send + Sync + 'static,
    {
        let descriptor = BlockDescriptor::new(device, block_size, block_count);
        self.devices.insert(id, Box::new(descriptor));
    }

    pub fn get(&self, id: &DeviceId) -> Option<&dyn BlockDevice> {
        self.devices.get(id).map(|d| d.as_ref())
    }

    pub fn count(&self) -> usize {
        self.devices.len()
    }
}

pub trait BlockDevice: Send + Sync {
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError>;
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockDeviceError>;
    fn flush(&self) -> Result<(), BlockDeviceError>;

    fn block_size(&self) -> u32;
    fn block_count(&self) -> u64;

    fn capacity(&self) -> u64 {
        self.block_size() as u64 * self.block_count()
    }
}

pub enum BlockDeviceError {
    IoError,
    InvalidRange { lba: u64, count: u64 },
    NotAligned,
    DeviceFault(String),
}

pub struct BlockDescriptor<T: StorageDevice> {
    device: Arc<Mutex<T>>,
    block_size: u32,
    block_count: u64,
    label: Option<String>,
}

impl<T: StorageDevice> BlockDescriptor<T> {
    pub fn new(device: Arc<Mutex<T>>, block_size: u32, block_count: u64) -> Self {
        Self {
            device,
            block_size,
            block_count,
            label: None,
        }
    }
}

impl<T: StorageDevice + Send + Sync> BlockDevice for BlockDescriptor<T> {
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), BlockDeviceError> {
        todo!()
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), BlockDeviceError> {
        todo!()
    }

    fn flush(&self) -> Result<(), BlockDeviceError> {
        todo!()
    }

    fn block_size(&self) -> u32 {
        self.block_size
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }
}

pub trait StorageDevice {
    type Error: core::fmt::Display;

    fn read_blocks(&mut self, lba: u64, count: u64, buf: &mut [u8]) -> Result<(), Self::Error>;
    fn write_blocks(&mut self, lba: u64, count: u64, buf: &[u8]) -> Result<(), Self::Error>;
    fn flush(&mut self) -> Result<(), Self::Error>;
}
