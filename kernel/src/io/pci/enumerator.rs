use core::array;

use alloc::{boxed::Box, string::String, vec::Vec};
use conquer_once::spin::OnceCell;
use log::warn;
use x86_64::{PhysAddr, instructions::port::Port};

use crate::{
    allocator::mmio::{MappedRegion, map_mmio},
    io::pci::{HeaderType, PciDevice},
    sys::acpi::sdt::mcfg::McfgEntry,
};

pub trait PciEnumerator {
    fn read_u8(&self, bus: u8, device: u8, function: u8, offset: u8) -> u8;
    fn read_u16(&self, bus: u8, device: u8, function: u8, offset: u8) -> u16;
    fn read_u32(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32;

    fn enumerate(&self) -> Vec<PciDevice> {
        let mut devices = Vec::new();
        let header_type = self.read_u8(0, 0, 0, PciDevice::OFF_HEADER);

        if !HeaderType::is_multi_function(header_type) {
            self.check_bus(&mut devices, 0);
        } else {
            for function in 1..8u8 {
                let vendor_id = self.read_u16(0, 0, function, PciDevice::OFF_ID);
                if vendor_id == 0xFFFF {
                    break;
                }
                self.check_bus(&mut devices, function);
            }
        }

        devices
    }

    fn check_bus(&self, devices: &mut Vec<PciDevice>, bus: u8) {
        for device_num in 0..32u8 {
            self.check_device(devices, bus, device_num);
        }
    }

    fn check_device(&self, devices: &mut Vec<PciDevice>, bus: u8, device_num: u8) {
        let id = self.read_u32(bus, device_num, 0, PciDevice::OFF_ID);
        if id == 0xFFFF_FFFF {
            return;
        }

        let header_raw = self.read_u8(bus, device_num, 0, PciDevice::OFF_HEADER);
        let function_count = if HeaderType::is_multi_function(header_raw) {
            8
        } else {
            1
        };

        for function in 0..function_count {
            let id = if function == 0 {
                id
            } else {
                self.read_u32(bus, device_num, function, PciDevice::OFF_ID)
            };
            if id == 0xFFFF_FFFF {
                continue;
            }

            self.check_function(devices, bus, device_num, function, id);
        }
    }

    fn check_function(
        &self,
        devices: &mut Vec<PciDevice>,
        bus: u8,
        device_num: u8,
        function: u8,
        id: u32,
    ) {
        let class = self.read_u8(bus, device_num, function, PciDevice::OFF_CLASS + 3);
        let subclass = self.read_u8(bus, device_num, function, PciDevice::OFF_CLASS + 2);

        if class == 0x06 && subclass == 0x04 {
            let sec_bus = self.read_u8(bus, device_num, function, PciDevice::OFF_SEC_BUS);
            self.check_bus(devices, sec_bus);
        }

        match self.make_device(id, bus, device_num, function) {
            Ok(dev) => devices.push(dev),
            Err(e) => warn!("Failed to init PCI device {:#x}: {}", id, e),
        }
    }

    fn make_device(&self, id: u32, bus: u8, device: u8, function: u8) -> Result<PciDevice, String> {
        PciDevice::new(id, bus, device, function)
    }
}

pub struct CpuIoEnumerator;

impl CpuIoEnumerator {
    const CONFIG_ADDRESS: u16 = 0xCF8;
    const CONFIG_DATA: u16 = 0xCFC;

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

    fn pci_read(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
        let address: u32 = Self::get_addr(bus, device, func, offset);

        let mut addr = Port::<u32>::new(Self::CONFIG_ADDRESS);
        let mut data = Port::<u32>::new(Self::CONFIG_DATA);

        unsafe {
            addr.write(address);
            data.read()
        }
    }
}

impl PciEnumerator for CpuIoEnumerator {
    fn read_u8(&self, bus: u8, device: u8, function: u8, offset: u8) -> u8 {
        (Self::pci_read(bus, device, function, offset) & 0xFF) as u8
    }

    fn read_u16(&self, bus: u8, device: u8, function: u8, offset: u8) -> u16 {
        (Self::pci_read(bus, device, function, offset) & 0xFFFF) as u16
    }

    fn read_u32(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        Self::pci_read(bus, device, function, offset)
    }
}

pub struct McfgEnumerator<'a> {
    entry: &'a McfgEntry,
    bus_regions: Box<[OnceCell<MappedRegion>]>,
}

impl<'a> McfgEnumerator<'a> {
    pub fn new(entry: &'a McfgEntry) -> Self {
        let bus_count = (entry.bus_num_end - entry.bus_num_start + 1) as usize;
        Self {
            entry,
            bus_regions: (0..bus_count).map(|_| OnceCell::uninit()).collect(),
        }
    }

    fn get_bus_region(&self, bus: u8) -> &MappedRegion {
        // 32 devices, 8 functions, 1 KB each
        const BUS_SIZE: u64 = 32 * 8 * 4096;

        let idx = (bus - self.entry.bus_num_start) as usize;
        self.bus_regions[idx].get_or_init(|| {
            let phys = self.entry.base_addr + (((bus - self.entry.bus_num_start) as u64) << 20);
            map_mmio(PhysAddr::new(phys), BUS_SIZE)
        })
    }

    fn read_raw(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        let region = self.get_bus_region(bus);

        let addr = region.virt().as_u64()
            + ((device as u64) << 15)
            + ((function as u64) << 12)
            + offset as u64;

        unsafe { *(addr as *const u32) }
    }
}

impl PciEnumerator for McfgEnumerator<'_> {
    fn read_u8(&self, bus: u8, device: u8, function: u8, offset: u8) -> u8 {
        (self.read_raw(bus, device, function, offset) & 0xFF) as u8
    }

    fn read_u16(&self, bus: u8, device: u8, function: u8, offset: u8) -> u16 {
        (self.read_raw(bus, device, function, offset) & 0xFFFF) as u16
    }

    fn read_u32(&self, bus: u8, device: u8, function: u8, offset: u8) -> u32 {
        self.read_raw(bus, device, function, offset)
    }
}
