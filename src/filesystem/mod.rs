use alloc::{sync::Arc, vec::Vec};
use spin::Mutex;

use crate::{filesystem::drivers::nvme::NVMe, pci::PciDevice, println};

pub mod drivers;

struct Ext2FileSystem {}

pub fn init() {
    let device = {
        let mut devices = crate::pci::DEVICES.lock();

        // https://wiki.osdev.org/PCI#Class_Codes
        let mut storage_devices: Vec<Arc<Mutex<PciDevice>>> = Vec::new();
        for device in devices.iter() {
            let binding = device.lock();
            if binding.class() != 0x1 {
                continue;
            }

            storage_devices.push(Arc::clone(device));
        }

        println!("devices: {:?}", storage_devices);

        let Some(device) = storage_devices.first() else {
            println!("Failed finding a suitable storage device!");
            return;
        };

        Arc::clone(device)
    };

    let driver = NVMe::new(device);
}
