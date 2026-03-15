use core::fmt;

use alloc::vec::Vec;
use x86_64::instructions::port::Port;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
}

impl fmt::Display for PciDevice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "PCI Device: bus={} device={} function={} vendor=0x{:04x} device=0x{:04x} class=0x{:02x} subclass=0x{:02x}",
            self.bus,
            self.device,
            self.function,
            self.vendor_id,
            self.device_id,
            self.class,
            self.subclass
        )
    }
}

pub fn check_all_buses() -> Vec<PciDevice> {
    let mut devices: Vec<PciDevice> = Vec::new();
    for bus in 0..=255 {
        for device in 0..32 {
            check_device(bus, device, &mut devices);
        }
    }

    devices
}

fn check_device(bus: u8, device: u8, devices: &mut Vec<PciDevice>) {
    // https://wiki.osdev.org/PCI#Common_Header_Fields
    if let Some(dev) = check_function(bus, device, 0) {
        devices.push(dev);
    } else {
        return;
    }

    let header = ((pci_read(bus, device, 0, 0xC) >> 16) & 0xFF) as u8;
    if (header & 0x80) != 0 {
        for function in 1..8 {
            if let Some(dev) = check_function(bus, device, function) {
                devices.push(dev);
            }
        }
    }
}

fn check_function(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    let id: u32 = pci_read(bus, device, function, 0x0);
    let vendor_id = (id & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return None;
    }

    let device_id = (id >> 16) as u16;

    let class_reg = pci_read(bus, device, function, 0x08);
    let class = ((class_reg >> 24) & 0xFF) as u8;
    let subclass = ((class_reg >> 16) & 0xFF) as u8;

    // https://wiki.osdev.org/PCI#Class_Codes
    Some(PciDevice {
        bus,
        device,
        function,
        vendor_id,
        device_id,
        class,
        subclass,
    })
}

// https://wiki.osdev.org/PCI#The_PCI_Bus
fn pci_read(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    let lbus = bus as u32;
    let ldevice = device as u32;
    let lfunc = func as u32;
    let loffset = offset as u32;

    // Bit 31	   Bits 30-24	Bits 23-16	Bits 15-11		Bits 10-8			Bits 7-0
    // Enable Bit	Reserved	Bus Number	Device Number	Function Number		Register Offset1

    let address: u32 =
        0x80000000 | (lbus << 16) | (ldevice << 11) | (lfunc << 8) | (loffset & 0xFC);

    let mut addr = Port::<u32>::new(CONFIG_ADDRESS);
    let mut data = Port::<u32>::new(CONFIG_DATA);

    unsafe {
        addr.write(address);
        data.read()
    }
}
