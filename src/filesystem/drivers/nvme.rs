use alloc::sync::Arc;
use spin::Mutex;

use crate::{filesystem::drivers::StorageDevice, pci::PciDevice, println};

pub struct NVMe {}

impl NVMe {
    pub fn new(guard: Arc<Mutex<PciDevice>>) -> Self {
        let binding = guard.lock();
        let Some(bar0) = PciDevice::get_bar(&binding, 0) else {
            panic!("Could not find BAR0 for NVMe!");
        };
        bar0.map_mmio();

        println!("bar0: {:?}", bar0);

        Self {}
    }
}

impl StorageDevice for NVMe {
    fn read() {
        todo!()
    }

    fn write() {
        todo!()
    }
}
