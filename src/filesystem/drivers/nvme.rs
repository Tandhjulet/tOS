use alloc::sync::Arc;
use spin::Mutex;

use crate::{filesystem::drivers::BlockDevice, pci::PciDevice};

pub struct NVMe {}

impl NVMe {
    pub fn new(guard: Arc<Mutex<PciDevice>>) -> Self {
        let mut bar0 = PciDevice::get_bar(&guard.lock(), 0);

        Self {}
    }
}

impl BlockDevice for NVMe {
    fn read() {
        todo!()
    }

    fn write() {
        todo!()
    }
}
