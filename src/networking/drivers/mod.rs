/**
 * See https://wiki.osdev.org/Intel_Ethernet_i217 for documentation
 * details regarding the implementation of the E1000 driver.
 */
use core::ptr::{read_volatile, write_volatile};

use x86_64::{PhysAddr, instructions::port::Port};

use crate::{
    allocator::{self, mmio},
    networking::NetworkDriver,
    pci::PciDevice,
    print, println,
};

const E1000_NUM_RX_DESC: usize = 32;
const E1000_NUM_TX_DESC: usize = 8;

const REG_EEPROM: u16 = 0x0014;
const BAR0_OFFSET: u8 = 0x10;
const BAR1_OFFSET: u8 = 0x14;

pub struct E1000RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

pub struct E1000TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

pub struct E1000 {
    bar_type: u8,
    io_base: u16,
    mem_base: u64,
    eeprom_exists: bool,
    mac: [u8; 6],
    rx_descs: [Option<E1000RxDesc>; E1000_NUM_RX_DESC],
    tx_descs: [Option<E1000TxDesc>; E1000_NUM_TX_DESC],
    rx_cur: u16,
    tx_cur: u16,
}

impl E1000 {
    pub fn new(device: &PciDevice) -> Self {
        // https://wiki.osdev.org/PCI#Base_Address_Registers
        let bar0 = device.read(BAR0_OFFSET);
        let bar_type = (bar0 & 0x1) as u8;

        let is_io_bar = bar_type == 1;

        // TOD: move this into the PciDevice impl
        let (io_bar_addr, mem_base): (u16, u64) = if is_io_bar {
            // I/O Space BAR layout
            // Bits 31-2						Bit 1		Bit 0
            // 4-Byte Aligned Base Address		Reserved	Always 1

            let io_bar_addr = (bar0 & 0xFFFFFFFC) as u16;
            (io_bar_addr, 0)
        } else {
            // Memory Space BAR layout:
            // Bits 31-4						Bit 3			Bits 2-1	Bit 0
            // 16-Byte Aligned Base Address		Prefetchable	Type		Always 0

            let mem_type = (bar0 >> 1) & 0x3;
            let mem_base;

            if mem_type == 0x2 {
                // 64-bit memory bar
                let bar_low = bar0 & 0xFFFFFFF0;
                let bar_high = device.read(BAR1_OFFSET);
                mem_base = ((bar_high as u64) << 32) | (bar_low as u64);
            } else {
                // 32-bit memory bar
                mem_base = (bar0 & 0xFFFFFFF0) as u64;
            }

            (0, mem_base)
        };

        if !is_io_bar {
            let phys_addr = PhysAddr::new(mem_base);
            mmio::map_mmio_region(phys_addr, virt_start, size);
        }

        // TODO: map phys -> virt addrs so we can access

        Self {
            bar_type,
            io_base: io_bar_addr,
            mem_base: mem_base,
            eeprom_exists: false,
            mac: [0; 6],
            rx_descs: [(); E1000_NUM_RX_DESC].map(|_| None),
            tx_descs: [(); E1000_NUM_TX_DESC].map(|_| None),
            rx_cur: 0,
            tx_cur: 0,
        }
    }

    fn write_command(&self, p_addr: u16, p_val: u32) {
        if self.bar_type == 0 {
            unsafe {
                let addr = self.mem_base + (p_addr as u64);
                let ptr = addr as *mut u32;
                write_volatile(ptr, p_val);
            }
        } else {
            unsafe {
                Port::<u32>::new(self.io_base).write(p_addr as u32);
                Port::<u32>::new(self.io_base + 4).write(p_val);
            };
        }
    }

    fn read_command(&self, p_addr: u16) -> u32 {
        if self.bar_type == 0 {
            let addr = self.mem_base + (p_addr as u64);
            unsafe {
                return read_volatile(addr as *const u32);
            }
        } else {
            unsafe {
                Port::<u32>::new(self.io_base).write(p_addr as u32);
                Port::<u32>::new(self.io_base + 4).read()
            }
        }
    }

    fn detect_eeprom(&mut self) -> bool {
        self.write_command(REG_EEPROM, 0x1);

        self.eeprom_exists = false;
        for _ in 0..1000 {
            if self.read_command(REG_EEPROM) & 0x10 > 0 {
                self.eeprom_exists = true;
            }
        }

        return self.eeprom_exists;
    }

    fn read_eeprom(&self, addr: u8) -> u16 {
        let mut tmp: u32;

        if self.eeprom_exists {
            self.write_command(REG_EEPROM, 1 | ((addr as u32) << 8));
            while {
                tmp = self.read_command(REG_EEPROM);
                tmp & (1 << 4) == 0
            } {}
        } else {
            self.write_command(REG_EEPROM, 1 | ((addr as u32) << 2));
            while {
                tmp = self.read_command(REG_EEPROM);
                tmp & (1 << 1) == 0
            } {}
        }

        return ((tmp >> 16) & 0xFFFF) as u16;
    }

    fn read_mac(&mut self) -> bool {
        if self.eeprom_exists {
            let mut tmp: u16 = self.read_eeprom(0);
            self.mac[0] = (tmp & 0xff) as u8;
            self.mac[1] = (tmp >> 8) as u8;

            tmp = self.read_eeprom(1);
            self.mac[2] = (tmp & 0xff) as u8;
            self.mac[3] = (tmp >> 8) as u8;

            tmp = self.read_eeprom(2);
            self.mac[4] = (tmp & 0xff) as u8;
            self.mac[5] = (tmp >> 8) as u8;
            return true;
        }

        let mem_base_mac8: *const u8 = (self.mem_base + 0x5400) as *const u8;
        let mem_base_mac32: *const u32 = (self.mem_base + 0x5400) as *const u32;

        unsafe {
            if (*mem_base_mac32) == 0 {
                return false;
            }

            for i in 0..6 {
                self.mac[i] = *mem_base_mac8.add(i);
            }
        }

        return true;
    }
}

impl NetworkDriver for E1000 {
    fn start(&mut self) {
        self.detect_eeprom();
        if !self.read_mac() {
            return;
        }

        for i in 0..6 {
            print!("{}", self.mac[i]);
        }
    }

    fn fire(&mut self, frame: x86_64::structures::idt::InterruptStackFrame) {
        todo!()
    }

    fn get_mac_addr(&self) -> [u8; 6] {
        todo!()
    }

    fn send_packet(&mut self, data: &[u8]) -> Result<(), &'static str> {
        todo!()
    }
}
