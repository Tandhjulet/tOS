use alloc::string::ToString;

use crate::println;

pub mod pci;

pub fn scan_buses() {
    let pci_devices = pci::check_all_buses();
    for device in pci_devices {
        println!("{}", device.to_string());
    }
}
