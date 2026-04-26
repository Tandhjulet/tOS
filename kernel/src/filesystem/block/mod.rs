use core::fmt::Display;

use alloc::{
    boxed::Box, collections::btree_map::BTreeMap, format, string::String, sync::Arc, vec::Vec,
};
use spin::Mutex;

pub mod nvme;

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
    type_counters: BTreeMap<&'static str, usize>,
}

impl BlockDeviceRegistry {
    pub fn new() -> Self {
        Self {
            devices: BTreeMap::new(),
            type_counters: BTreeMap::new(),
        }
    }

    pub fn register<T>(
        &mut self,
        type_name: &'static str,
        device: Arc<Mutex<T>>,
        block_size: u32,
        block_count: u64,
    ) -> DeviceId
    where
        T: StorageDevice + Send + Sync + 'static,
    {
        let counter = self.type_counters.entry(type_name).or_insert(0);
        let id = DeviceId::new(format!("{}{}", type_name, counter));
        *counter += 1;

        let descriptor = BlockDescriptor::new(device, block_size, block_count);
        self.devices.insert(id.clone(), Box::new(descriptor));
        id
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
