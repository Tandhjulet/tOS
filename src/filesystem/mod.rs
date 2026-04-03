use alloc::{sync::Arc, vec::Vec};
use spin::Mutex;

use crate::{filesystem::drivers::nvme::NVMeController, pci::PciDevice, println};

pub mod drivers;

struct Ext2FileSystem {}

pub fn init() {
    let devices = {
        let mut devices = crate::pci::DEVICES.lock();

        // https://wiki.osdev.org/PCI#Class_Codes
        let storage_devices = devices
            .iter_mut()
            .filter(|d| {
                let d = d.lock();
                d.class() == 0x1 && d.subclass() == 0x8
            })
            .map(|d| Arc::clone(d))
            .collect::<Vec<Arc<Mutex<PciDevice>>>>();

        storage_devices
    };

    for device in devices {
        let driver = NVMeController::new(device);
    }
}
