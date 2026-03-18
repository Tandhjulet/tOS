use core::alloc::{GlobalAlloc, Layout};

use x86_64::{
    PhysAddr, VirtAddr, addr,
    structures::{idt::InterruptStackFrame, paging::Translate},
};

/**
 * See https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf#page389
 * or (https://wiki.osdev.org/Intel_Ethernet_i217) for documentation
 * details regarding the implementation of the E1000 driver.
 */
use crate::{
    allocator::{self, ALLOCATOR, mmio},
    helpers,
    interrupts::IDT,
    networking::NetworkDriver,
    pci::{
        PciDevice,
        bar::{AnyBAR, BAR},
    },
    print, println,
};

const E1000_NUM_RX_DESC: usize = 32;
const E1000_NUM_TX_DESC: usize = 8;

// Refer to Table 13-2 in https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf#page389
// for documentation about these constants
const REG_CTRL: u16 = 0x0; // Device Control
const REG_STATUS: u16 = 0x0008; // Device Status
const REG_TXCW: u16 = 0x0178; // Transmit Configuration
const REG_EEPROM: u16 = 0x0014; // EEPROM Read

// RX/TX
const REG_R_CTRL: u16 = 0x0100; // Receive Control
const REG_RX_DESC_LO: u16 = 0x2800; // Receive Descriptor Base Low
const REG_RX_DESC_HI: u16 = 0x2804; // Receive Descriptor Base High

const REG_T_CTRL: u16 = 0x0400; // Transmit Control
const REG_TX_DESC_LO: u16 = 0x3800; // Transmit Descriptor Base Low
const REG_TX_DESC_HI: u16 = 0x3804; // Transmit Descriptor Base High

const REG_RX_DESC_LEN: u16 = 0x2808;

const REG_RX_DESC_HEAD: u16 = 0x2810;
const REG_RX_DESC_TAIL: u16 = 0x2818;

// RX FLAGS (see 13.4.22 intel docs)
const RCTL_EN: u32 = 1 << 1; // Enable Receiver
const RCTL_SBP: u32 = 1 << 2; // Store Bad Packets
const RCTL_UPE: u32 = 1 << 3; // Unicast Promiscuous Enabled
const RCTL_MPE: u32 = 1 << 4; // Multicast Promiscuous Enabled
const RCTL_LPE: u32 = 1 << 5; // Long Packet Reception Enable
// TODO: implement
const RCTL_BAM: u32 = 1 << 15; // Broadcast Accept Mode
// Buffer sizes
const RCTL_BSIZE_256: u32 = 3 << 16;
const RCTL_BSIZE_512: u32 = 2 << 16;
const RCTL_BSIZE_1024: u32 = 1 << 16;
const RCTL_BSIZE_2048: u32 = 0 << 16;
const RCTL_BSIZE_4096: u32 = (3 << 16) | (1 << 25);
const RCTL_BSIZE_8192: u32 = (2 << 16) | (1 << 25);
const RCTL_BSIZE_16384: u32 = (1 << 16) | (1 << 25);
const RCTL_VFE: u32 = 1 << 18; // VLAN Filter Enable
const RCTL_CFIEN: u32 = 1 << 19; // Canonical Form Indicator Enable
const RCTL_CFI: u32 = 1 << 20; // Canonical Form Indicator Bit Value
const RCTL_DPF: u32 = 1 << 22; // Discard Pause Frames
const RCTL_PMCF: u32 = 1 << 23; // Pass MAC Control Frames
const RCTL_SECRC: u32 = 1 << 26; // Strip Ethernet CRC

// INTERRUPTS
const REG_I_MASK: u16 = 0x00D0; // Interrupt Mask Set/Read
const REG_I_READ: u16 = 0xC0;

// MISC
const MTA_LENGTH: usize = 128;
const REG_MTA: [u16; MTA_LENGTH] = helpers::build_range::<MTA_LENGTH>(0x5200, 4); // Multicast Table Array 

const RX_BUFFER_SIZE: usize = 8192;

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct E1000RxDesc {
    addr: u64,
    length: u16,
    checksum: u16,
    status: u8,
    errors: u8,
    special: u16,
}

#[derive(Copy, Clone)]
#[repr(C, align(16))]
pub struct E1000TxDesc {
    addr: u64,
    length: u16,
    cso: u8,
    cmd: u8,
    status: u8,
    css: u8,
    special: u16,
}

impl Default for E1000TxDesc {
    fn default() -> Self {
        Self {
            addr: 0,
            length: 0,
            cso: 0,
            cmd: 0,
            status: 0,
            css: 0,
            special: 0,
        }
    }
}

impl Default for E1000RxDesc {
    fn default() -> Self {
        Self {
            addr: 0,
            length: 0,
            checksum: 0,
            status: 0,
            errors: 0,
            special: 0,
        }
    }
}

pub struct E1000<'a> {
    device: &'a PciDevice,
    bar0: AnyBAR<'a>,
    eeprom_exists: bool,
    mac: [u8; 6],
    rx_descs: [E1000RxDesc; E1000_NUM_RX_DESC],
    tx_descs: [E1000TxDesc; E1000_NUM_TX_DESC],
    rx_cur: u16,
    tx_cur: u16,
}

impl<'a> E1000<'a> {
    pub fn new(device: &'a PciDevice) -> Self {
        // https://wiki.osdev.org/PCI#Base_Address_Registers

        let mut bar0 = device.get_bar(0);
        if let AnyBAR::Mem(ref mut bar) = bar0 {
            let virt_addr = {
                let phys_addr = bar.addr();

                let size = bar.size() as u64;
                mmio::map_mmio(phys_addr, size)
            };

            bar.set_virt_addr(virt_addr);
        }

        // TODO: enable bus mastering (?)

        Self {
            device,
            bar0,
            eeprom_exists: false,
            mac: [0; 6],
            rx_descs: [E1000RxDesc::default(); E1000_NUM_RX_DESC],
            tx_descs: [E1000TxDesc::default(); E1000_NUM_TX_DESC],
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

    pub fn clear_mta(&mut self) {
        for addr in REG_MTA {
            unsafe {
                self.bar0.write_command(addr, 0x0);
            };
        }
    }

    pub fn enable_interrupts(&mut self) {
        unsafe {
            self.bar0.write_command(REG_I_MASK, 0x1F6DC);
            self.bar0.write_command(REG_I_MASK, 0xFF & !4);

            // read to clear all reg bits
            self.bar0.read_command(REG_I_READ);
        }
    }

    pub fn rx_init(&mut self) {
        let guard = allocator::MAPPER.lock();
        let mapper = guard.as_ref().unwrap();

        let raw_rx_addr = self.rx_descs.as_ptr() as u64;
        let phys_rx_addr = mapper.translate_addr(VirtAddr::new(raw_rx_addr)).unwrap();
        let raw_phys_rx = phys_rx_addr.as_u64();

        let buf_layout = Layout::from_size_align(RX_BUFFER_SIZE, 16).unwrap();
        for i in 0..E1000_NUM_RX_DESC {
            let buf_ptr: *mut u8 = unsafe { ALLOCATOR.alloc(buf_layout) };
            assert!(!buf_ptr.is_null(), "Failed to allocate RX buffer");

            let buf_phys = mapper
                .translate_addr(VirtAddr::new(buf_ptr as u64))
                .unwrap();

            let desc = &mut self.rx_descs[i];
            desc.addr = buf_phys.as_u64();
        }

        unsafe {
            // FIXME: why is this reversed?
            self.bar0
                .write_command(REG_TX_DESC_LO, (raw_phys_rx >> 32) as u32);
            self.bar0
                .write_command(REG_TX_DESC_HI, (raw_phys_rx & 0xFFFF_FFFF) as u32);

            // IDK ? pr intel docs it should be akin to upper
            self.bar0.write_command(REG_RX_DESC_LO, raw_phys_rx as u32);
            self.bar0.write_command(REG_RX_DESC_HI, 0x0);

            self.bar0
                .write_command(REG_RX_DESC_LEN, (E1000_NUM_RX_DESC * 16) as u32);

            self.bar0.write_command(REG_RX_DESC_HEAD, 0);
            self.bar0
                .write_command(REG_RX_DESC_TAIL, (E1000_NUM_RX_DESC - 1) as u32);

            // TODO: define and use flags
            let flags = 0x0;
            self.bar0.write_command(REG_R_CTRL, flags);
        };
        // WRIE TO 0x0100h BSIZE (bits 17:16) that RX buffer size = 8192 bytes (write 10b)
    }

    extern "x86-interrupt" fn fire(stack_frame: InterruptStackFrame) {
        println!("interrupt registered!");
    }
}

impl NetworkDriver for E1000<'_> {
    fn start(&mut self) {
        self.detect_eeprom();
        if !self.read_mac() {
            return;
        }

        self.print_mac();
        self.clear_mta();

        let interrupt_line = self.device.interrupt_line;
        IDT.lock()[usize::from(interrupt_line)].set_handler_fn(E1000::fire);

        self.enable_interrupts();
        self.rx_init();
    }

    fn get_mac_addr(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, data: &[u8]) -> Result<(), &'static str> {
        todo!()
    }
}
