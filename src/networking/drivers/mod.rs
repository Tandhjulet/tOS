/**
 * See https://wiki.osdev.org/Intel_Ethernet_i217 for documentation
 * details regarding the implementation of the E1000 driver.
 */
use x86_64::PhysAddr;

use crate::{
    allocator::mmio,
    networking::NetworkDriver,
    pci::{
        PciDevice,
        bar::{AnyBAR, BAR},
    },
    print, println,
};

const E1000_NUM_RX_DESC: usize = 32;
const E1000_NUM_TX_DESC: usize = 8;

const REG_EEPROM: u16 = 0x0014;

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

pub struct E1000<'a> {
    bar0: AnyBAR<'a>,
    eeprom_exists: bool,
    mac: [u8; 6],
    rx_descs: [Option<E1000RxDesc>; E1000_NUM_RX_DESC],
    tx_descs: [Option<E1000TxDesc>; E1000_NUM_TX_DESC],
    rx_cur: u16,
    tx_cur: u16,
}

impl<'a> E1000<'a> {
    pub fn new(device: &'a PciDevice) -> Self {
        // https://wiki.osdev.org/PCI#Base_Address_Registers

        let mut bar0 = device.get_bar(0);
        if let AnyBAR::Mem(ref mut mem) = bar0 {
            let virt_addr = {
                let phys_addr = mem.addr();

                let size = mem.size() as u64;
                mmio::map_mmio(phys_addr, size)
            };

            mem.set_virt_addr(virt_addr);
        }

        Self {
            bar0,
            eeprom_exists: false,
            mac: [0; 6],
            rx_descs: [(); E1000_NUM_RX_DESC].map(|_| None),
            tx_descs: [(); E1000_NUM_TX_DESC].map(|_| None),
            rx_cur: 0,
            tx_cur: 0,
        }
    }

    fn detect_eeprom(&mut self) -> bool {
        unsafe { self.bar0.write_command(REG_EEPROM, 0x1) };

        self.eeprom_exists = false;
        for _ in 0..1000 {
            if unsafe { self.bar0.read_command(REG_EEPROM) } & 0x10 > 0 {
                self.eeprom_exists = true;
            }
        }

        return self.eeprom_exists;
    }

    fn read_eeprom(&mut self, addr: u8) -> u16 {
        let mut tmp: u32;

        if self.eeprom_exists {
            unsafe {
                self.bar0
                    .write_command(REG_EEPROM, 1 | ((addr as u32) << 8))
            };
            while {
                tmp = unsafe { self.bar0.read_command(REG_EEPROM) };
                tmp & (1 << 4) == 0
            } {}
        } else {
            unsafe {
                self.bar0
                    .write_command(REG_EEPROM, 1 | ((addr as u32) << 2))
            };
            while {
                tmp = unsafe { self.bar0.read_command(REG_EEPROM) };
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

        match &self.bar0 {
            AnyBAR::IO(_) => {
                println!("ERROR: Trying to read the MAC of IO BAR without an EEPROM!");
                return false;
            }
            AnyBAR::Mem(mem) => {
                let mem_base_mac8: *const u8 = (mem.virt_addr().as_u64() + 0x5400) as *const u8;
                let mem_base_mac32: *const u32 = (mem.virt_addr().as_u64() + 0x5400) as *const u32;

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
    }

    fn print_mac(&mut self) {
        for i in 0..6 {
            if i != 0 {
                print!(":");
            }
            print!("{:02X}", self.mac[i]);
        }
        println!();
    }
}

impl NetworkDriver for E1000<'_> {
    fn start(&mut self) {
        self.detect_eeprom();
        if !self.read_mac() {
            return;
        }

        self.print_mac();
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
