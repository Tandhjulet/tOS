pub mod bar;

use core::fmt;

use alloc::{format, string::String, sync::Arc, vec::Vec};
use lazy_static::lazy_static;
use num_enum::TryFromPrimitive;
use spin::Mutex;
use x86_64::instructions::port::Port;

use crate::pci::bar::Bar;

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

lazy_static! {
    pub static ref DEVICES: Mutex<Vec<Arc<Mutex<PciDevice>>>> = Mutex::new(check_all_buses());
}

//
// Command Register flags
// (See https://wiki.osdev.org/PCI#Command_Register for overview)
//
pub mod cmd {
    pub const IO: u16 = 1 << 0; // Enable IO Space accesses
    pub const MEM: u16 = 1 << 1; // Enable MEM Space access
    pub const BUS_MASTER: u16 = 1 << 2; // Enable Bus Mastering
    pub const SC: u16 = 1 << 3; // Enable monitor for Special Cycle ops
    pub const MEM_WRITE: u16 = 1 << 4; // Enable the device to generate mem write and invalidate commands
    pub const VGA_PALETTE: u16 = 1 << 5; // If set, the device will not respond to palette reg writes but instead snoop the data
    pub const PARITY_ERR: u16 = 1 << 6; // If set, the device will take normal action when parity err is detected
    pub const SERR: u16 = 1 << 8; // If set, SERR# driver is enabled
    pub const FBBE: u16 = 1 << 9; // If set, the device can generate fast back-to-back transactions
    pub const INTERRUPT: u16 = 1 << 10; // If set, the devices INTx# signal is disabled
}

#[derive(Debug)]
pub struct PciDevice {
    bus: u8,
    device: u8,
    function: u8,

    vendor_id: u16,
    device_id: u16,
    class: u8,
    subclass: u8,

    bars: Vec<Option<Bar>>,
    header_type: HeaderType,
}

#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(u8)]
pub enum HeaderType {
    General = 0x0,  // endpoint devicee
    PciToPci = 0x1, // bridge
    CardBus = 0x2,
}

impl HeaderType {
    fn bar_count(self) -> usize {
        match self {
            HeaderType::General => 6,
            HeaderType::PciToPci => 2,
            _ => 0,
        }
    }

    pub fn is_multi_function(raw: u8) -> bool {
        raw & 0x80 != 0
    }
}

impl PciDevice {
    const OFF_ID: u8 = 0x0;
    const OFF_COMMAND_STATUS: u8 = 0x04;
    const OFF_CLASS: u8 = 0x08;
    const OFF_HEADER: u8 = 0x0C;
    const OFF_INTERRUPT: u8 = 0x3C;
    const OFF_BARS_START: u8 = 0x10;

    pub fn new(id: u32, bus: u8, device: u8, function: u8) -> Result<Self, String> {
        let vendor_id = (id & 0xFFFF) as u16;
        let device_id = (id >> 16) as u16;

        let class_reg = pci_read(bus, device, function, Self::OFF_CLASS);
        let class = (class_reg >> 24) as u8;
        let subclass = (class_reg >> 16) as u8;

        let header_raw = (pci_read(bus, device, function, Self::OFF_CLASS) >> 16) as u8;
        let header_type = HeaderType::try_from(header_raw)
            .map_err(|err| format!("failed to map {:#x} to a header type", err.number))?;

        let mut device = Self {
            bus,
            device,
            function,
            bars: Vec::with_capacity(header_type.bar_count()),
            vendor_id,
            device_id,
            class,
            subclass,
            header_type,
        };
        device.enumerate_bars();

        Ok(device)
    }

    pub fn read(&self, offset: u8) -> u32 {
        pci_read(self.bus, self.device, self.function, offset)
    }

    pub fn write(&self, offset: u8, value: u32) {
        pci_write(self.bus, self.device, self.function, offset, value);
    }

    pub fn get_bar(&self, index: usize) -> Option<&Bar> {
        self.bars.get(index)?.as_ref()
    }

    pub fn header_type(&self) -> HeaderType {
        self.header_type
    }

    pub fn bus(&self) -> u8 {
        self.bus
    }

    pub fn device(&self) -> u8 {
        self.device
    }

    pub fn function(&self) -> u8 {
        self.function
    }

    pub fn vendor_id(&self) -> u16 {
        self.vendor_id
    }

    pub fn device_id(&self) -> u16 {
        self.device_id
    }

    pub fn class(&self) -> u8 {
        self.class
    }

    pub fn subclass(&self) -> u8 {
        self.subclass
    }

    pub fn command(&self) -> u16 {
        (self.read(Self::OFF_COMMAND_STATUS) & 0xFFFF) as u16
    }

    pub fn status(&self) -> u16 {
        (self.read(Self::OFF_COMMAND_STATUS) >> 16) as u16
    }

    pub fn interrupt_line(&self) -> u8 {
        (self.read(Self::OFF_INTERRUPT) & 0xFF) as u8
    }

    pub fn interrupt_pin(&self) -> u8 {
        ((self.read(Self::OFF_INTERRUPT) >> 8) & 0xFF) as u8
    }

    pub fn set_command(&self, cmd: u16) {
        let val = (self.status() as u32) << 16 | cmd as u32;
        self.write(Self::OFF_COMMAND_STATUS, val);
    }

    pub fn enable_bus_mastering(&self) {
        self.set_command(self.command() | cmd::BUS_MASTER);
    }

    fn enumerate_bars(&mut self) {
        let mut slot = 0;
        while slot < self.header_type.bar_count() {
            let offset = Self::OFF_BARS_START + (slot as u8 * 4);
            let raw = self.read(offset);

            if raw == 0 || raw == 0xFFFF_FFFF {
                self.bars.push(None);
                slot += 1;
                continue;
            }

            let (bar, slots_used) = Bar::from_cfg(&self, offset);

            self.bars.push(Some(bar));
            for _ in 1..slots_used {
                self.bars.push(None);
            }

            slot += slots_used as usize;
        }
    }
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

fn check_all_buses() -> Vec<Arc<Mutex<PciDevice>>> {
    let mut devices: Vec<Arc<Mutex<PciDevice>>> = Vec::new();
    for bus in 0..=255 {
        for device in 0..32 {
            check_device(bus, device, &mut devices);
        }
    }

    devices
}

fn check_device(bus: u8, device: u8, devices: &mut Vec<Arc<Mutex<PciDevice>>>) {
    // https://wiki.osdev.org/PCI#Common_Header_Fields
    if let Some(dev) = check_function(bus, device, 0) {
        devices.push(Arc::new(Mutex::new(dev)));
    } else {
        return;
    }

    let header = ((pci_read(bus, device, 0, PciDevice::OFF_HEADER) >> 16) & 0xFF) as u8;
    if (header & 0x80) != 0 {
        for function in 1..8 {
            if let Some(dev) = check_function(bus, device, function) {
                devices.push(Arc::new(Mutex::new(dev)));
            }
        }
    }
}

fn check_function(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    let id: u32 = pci_read(bus, device, function, PciDevice::OFF_ID);
    let vendor_id = (id & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return None;
    }

    PciDevice::new(id, bus, device, function).ok()
}

fn get_addr(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    let lbus = bus as u32;
    let ldevice = device as u32;
    let lfunc = func as u32;
    let loffset = offset as u32;

    // Bit 31	   Bits 30-24	Bits 23-16	Bits 15-11		Bits 10-8			Bits 7-0
    // Enable Bit	Reserved	Bus Number	Device Number	Function Number		Register Offset1
    let address: u32 =
        0x80000000 | (lbus << 16) | (ldevice << 11) | (lfunc << 8) | (loffset & 0xFC);

    address
}

// https://wiki.osdev.org/PCI#The_PCI_Bus
fn pci_read(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    let address: u32 = get_addr(bus, device, func, offset);

    let mut addr = Port::<u32>::new(CONFIG_ADDRESS);
    let mut data = Port::<u32>::new(CONFIG_DATA);

    unsafe {
        addr.write(address);
        data.read()
    }
}

fn pci_write(bus: u8, device: u8, func: u8, offset: u8, value: u32) {
    let address: u32 = get_addr(bus, device, func, offset);

    let mut addr = Port::<u32>::new(CONFIG_ADDRESS);
    let mut data = Port::<u32>::new(CONFIG_DATA);

    unsafe {
        addr.write(address);
        data.write(value);
    }
}
