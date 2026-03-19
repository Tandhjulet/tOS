pub mod drivers;

use crate::networking::drivers::E1000;

pub fn init() {
    let mut devices = crate::pci::DEVICES.lock();

    // https://wiki.osdev.org/PCI#Class_Codes
    let device = devices
        .iter_mut()
        .find(|d| d.class == 0x2 && d.subclass == 0x0);

    let Some(device) = device else { return };

    // TODO: dynamic load
    let mut driver = E1000::new(device);
    driver.start();
}

pub trait NetworkDriver {
    fn start(&mut self);
    fn get_mac_addr(&self) -> [u8; 6];
    fn send_packet(&mut self, data: &[u8]) -> Result<(), &'static str>;
}
