pub mod bar;

use core::fmt;

use alloc::vec::Vec;
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::port::Port;

use crate::{pci::bar::AnyBAR, print};

const CONFIG_ADDRESS: u16 = 0xCF8;
const CONFIG_DATA: u16 = 0xCFC;

const BAR_OFFSETS: &[u8] = &[0x10, 0x14, 0x18, 0x1C, 0x20, 0x24];

lazy_static! {
    pub static ref DEVICES: Mutex<Vec<PciDevice>> = Mutex::new(check_all_buses());
}

// See https://wiki.osdev.org/PCI#Header_Type_0x0
const HEADER0_LINE0_OFFSET: u8 = 0x0;
const HEADER0_LINE1_OFFSET: u8 = 0x4;
const HEADER0_LINE2_OFFSET: u8 = 0x8;
const HEADER0_LINE3_OFFSET: u8 = 0xC;
const HEADER0_LINE15_OFFSET: u8 = 0x3C;

//
// Command Register flags
// (See https://wiki.osdev.org/PCI#Command_Register for overview)
//
const CMD_IO: u16 = 1 << 0; // Enable IO Space accesses
const CMD_MEM: u16 = 1 << 1; // Enable MEM Space access
const CMD_BUS_MASTER: u16 = 1 << 2; // Enable Bus Mastering
const CMD_SC: u16 = 1 << 3; // Enable monitor for Special Cycle ops
const CMD_MEM_WRITE: u16 = 1 << 4; // Enable the device to generate mem write and invalidate commands
const CMD_VGA_PALETTE: u16 = 1 << 5; // If set, the device will not respond to palette reg writes but instead snoop the data
const CMD_PARITY_ERR: u16 = 1 << 6; // If set, the device will take normal action when parity err is detected
const CMD_SERR: u16 = 1 << 8; // If set, SERR# driver is enabled
const CMD_FBBE: u16 = 1 << 9; // If set, the device can generate fast back-to-back transactions
const CMD_INTERRUPT: u16 = 1 << 10; // If set, the devices INTx# signal is disabled

#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,

    // Reg 0x0 @ Offset 0x0
    pub vendor_id: u16,
    pub device_id: u16,

    // Reg 0x1 @ Offset 0x4
    pub status: u16,
    pub command: u16,

    // Reg 0x2 @ Offset 0x8
    pub class: u8,
    pub subclass: u8,

    // Reg 0x3 @ Offset 0xC
    pub header_type: u8,
    pub latency: u8,
    pub cache_line_size: u8,
    pub bist: u8,

    // Reg 0xF @ 0x3C
    pub interrupt_pin: u8,
    pub interrupt_line: u8,
}

impl PciDevice {
    pub fn read(&self, offset: u8) -> u32 {
        pci_read(self.bus, self.device, self.function, offset)
    }

    pub fn write(&self, offset: u8, value: u32) {
        pci_write(self.bus, self.device, self.function, offset, value);
    }

    pub fn bar_count(&self) -> u8 {
        // https://wiki.osdev.org/PCI#Header_Type_0x0
        if self.header_type == 0x0 {
            return 6;
        } else if self.header_type == 0x1 {
            return 2;
        }

        return 0;
    }

    pub fn get_bar_offset(&self, bar: usize) -> u8 {
        return BAR_OFFSETS[bar];
    }

    pub fn get_bar(&self, bar: u8) -> AnyBAR {
        let bar_cnt = self.bar_count();

        // invalid bar count
        if bar > bar_cnt - 1 {
            panic!(
                "Invalid! BAR{} doesnt exist for type 0x{:04x}",
                bar, self.header_type
            );
        }

        let offset = self.get_bar_offset(bar as usize);
        let bar = self.read(offset);

        AnyBAR::from_raw(bar, offset, self)
    }

    pub fn enable_bus_mastering(&mut self) {
        self.command |= CMD_BUS_MASTER;

        let val = ((self.status as u32) << 16) | (self.command as u32);
        self.write(HEADER0_LINE1_OFFSET, val);
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

fn check_all_buses() -> Vec<PciDevice> {
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

    let header = ((pci_read(bus, device, 0, HEADER0_LINE3_OFFSET) >> 16) & 0xFF) as u8;
    if (header & 0x80) != 0 {
        for function in 1..8 {
            if let Some(dev) = check_function(bus, device, function) {
                devices.push(dev);
            }
        }
    }
}

fn check_function(bus: u8, device: u8, function: u8) -> Option<PciDevice> {
    let id: u32 = pci_read(bus, device, function, HEADER0_LINE0_OFFSET);
    let vendor_id = (id & 0xFFFF) as u16;
    if vendor_id == 0xFFFF {
        return None;
    }

    let device_id = (id >> 16) as u16;

    let class_reg = pci_read(bus, device, function, HEADER0_LINE1_OFFSET);
    let command = (class_reg & 0xFFFF) as u16;
    let status = (class_reg >> 16) as u16;

    let class_reg = pci_read(bus, device, function, HEADER0_LINE2_OFFSET);
    let class = ((class_reg >> 24) & 0xFF) as u8;
    let subclass = ((class_reg >> 16) & 0xFF) as u8;

    let line3 = pci_read(bus, device, function, HEADER0_LINE3_OFFSET);
    let (bist, header_type, latency, cache_line_size) = unpack_quad(line3);

    // Line 0xF always contains interrupt pin and interrupts line
    let line15 = pci_read(bus, device, function, HEADER0_LINE15_OFFSET);
    print!("line15: 0x{:04x} ({}: {})", line15, class, subclass);
    let (_, _, interrupt_pin, interrupt_line) = unpack_quad(line15);

    Some(PciDevice {
        bus,
        device,
        command,
        status,
        function,
        vendor_id,
        device_id,
        class,
        subclass,
        cache_line_size,
        latency,
        header_type,
        bist,
        interrupt_line,
        interrupt_pin,
    })
}

fn unpack_quad(val: u32) -> (u8, u8, u8, u8) {
    let first = (val & 0xFF) as u8;
    let second = ((val >> 8) & 0xFF) as u8;
    let third = ((val >> 16) & 0xFF) as u8;
    let fourth = ((val >> 24) & 0xFF) as u8;

    (fourth, third, second, first)
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
