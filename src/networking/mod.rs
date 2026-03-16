pub mod drivers;

use x86_64::structures::idt::InterruptStackFrame;

use crate::networking::drivers::E1000;

pub fn init() {
    let pci_devices = &*crate::pci::DEVICES;
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
    fn fire(&mut self, frame: InterruptStackFrame);
    fn get_mac_addr(&self) -> [u8; 6];
    fn send_packet(&mut self, data: &[u8]) -> Result<(), &'static str>;
}
