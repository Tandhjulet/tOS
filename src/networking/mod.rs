pub mod drivers;

use crate::networking::drivers::E1000;

pub fn init() {
    let pci_devices = &crate::pci::DEVICES;

    // https://wiki.osdev.org/PCI#Class_Codes
    let network_controller: Option<&crate::pci::PciDevice> = pci_devices
        .iter()
        .find(|device| device.class == 0x2 && device.subclass == 0x0);

    if network_controller.is_none() {
        return;
    }

    let device = network_controller.unwrap();
    // TODO: dynamic load
    let mut driver = E1000::new(device);
    driver.start();
}

pub trait NetworkDriver {
    fn start(&mut self);
    fn get_mac_addr(&self) -> [u8; 6];
    fn send_packet(&mut self, data: &[u8]) -> Result<(), &'static str>;
}
