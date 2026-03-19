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
    interrupts::{IDT, MIN_INTERRUPT},
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

const REG_RX_DESC_LEN: u16 = 0x2808;
const REG_RX_DESC_HEAD: u16 = 0x2810;
const REG_RX_DESC_TAIL: u16 = 0x2818;

const REG_T_CTRL: u16 = 0x0400; // Transmit Control
const REG_TX_DESC_LO: u16 = 0x3800; // Transmit Descriptor Base Low
const REG_TX_DESC_HI: u16 = 0x3804; // Transmit Descriptor Base High

const REG_TX_DESC_LEN: u16 = 0x3808;
const REG_TX_DESC_HEAD: u16 = 0x3810;
const REG_TX_DESC_TAIL: u16 = 0x3818;

//
// REG_R_CTRL (Receive Control) FLAGS (see 13.4.22 intel docs)
//
const RCTL_EN: u32 = 1 << 1; // Enable Receiver
const RCTL_SBP: u32 = 1 << 2; // Store Bad Packets
const RCTL_UPE: u32 = 1 << 3; // Unicast Promiscuous Enabled
const RCTL_MPE: u32 = 1 << 4; // Multicast Promiscuous Enabled
const RCTL_LPE: u32 = 1 << 5; // Long Packet Reception Enable
// Loopback Mode
const RCTL_LBM_NONE: u32 = 0 << 6; // No Loopback
const RCTL_LBM_PHY: u32 = 0b11 << 6; // PHY or external SerDesc loopback
// Receive Descriptor Minimum Threshold Size
const RCTL_RDMTS_HALF: u32 = 0 << 8; // Threshold is 1/2 of total RX circular desc buffer
const RCTL_RDMTS_QUARTER: u32 = 1 << 8; // Threshold is 1/4 of total RX circular desc buffer
const RCTL_RDMTS_EIGHT: u32 = 2 << 8; // Threshold is 1/8 of total RX circular desc buffer
// Multicast Offset
const RCTL_MO_36: u32 = 0 << 12; // Use bits 47:36
const RCTL_MO_35: u32 = 1 << 12; // Use bits 47:35
const RCTL_MO_34: u32 = 2 << 12; // Use bits 47:34
const RCTL_MO_32: u32 = 3 << 12; // Use bits 47:32
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

//
// TCTL (Transmit Control) FLAGS (See 13.4.33 Intel docs)
//
const TCTL_EN: u32 = 0x1 << 1; // Transmit Enable
const TCTL_PSP: u32 = 0x1 << 3; // Pad Short Packets
const TCTL_CT_SHIFT: u32 = 4; // Collision Threshold
const TCTL_COLD_SHIFT: u32 = 12; // Collision Distance
const TCTL_SWXOFF: u32 = 1 << 22; // Software XOFF Transmission
const TCTL_RTLC: u32 = 1 << 24; // Re-transmit on Late Collision

const TSTA_DD: u8 = 1 << 0; // Descriptor Done
const TSTA_EC: u8 = 1 << 1; // Express Collisions
const TSTA_LC: u8 = 1 << 2; // Late Collision
const TSTA_TU: u8 = 1 << 3; // Transmit Underrun

const CMD_EOP: u8 = 1 << 0;
const CMD_IFCS: u8 = 1 << 1;
const CMD_RS: u8 = 1 << 3;

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
    pub fn new(device: &'a mut PciDevice) -> Self {
        // enable bus mastering to enable DMA so the NIC can access receive and transmit descriptors
        device.enable_bus_mastering();

        let mut bar0 = device.get_bar(0);
        if let AnyBAR::Mem(ref mut bar) = bar0 {
            let virt_addr = {
                let phys_addr = bar.addr();

                let size = bar.size() as u64;
                mmio::map_mmio(phys_addr, size)
            };

            bar.set_virt_addr(virt_addr);
        }

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
                break;
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
            desc.status = 0;
        }

        unsafe {
            self.bar0
                .write_command(REG_RX_DESC_LO, (raw_phys_rx & 0xFFFF_FFFF) as u32);
            self.bar0
                .write_command(REG_RX_DESC_HI, (raw_phys_rx >> 32) as u32);

            let rx_desc_size = size_of::<E1000RxDesc>();
            self.bar0
                .write_command(REG_RX_DESC_LEN, (E1000_NUM_RX_DESC * rx_desc_size) as u32);

            self.bar0.write_command(REG_RX_DESC_HEAD, 0);
            self.bar0
                .write_command(REG_RX_DESC_TAIL, (E1000_NUM_RX_DESC - 1) as u32);

            let flags = 0x0
                | RCTL_EN
                | RCTL_SBP
                | RCTL_UPE
                | RCTL_MPE
                | RCTL_LBM_NONE
                | RCTL_RDMTS_HALF
                | RCTL_BAM
                | RCTL_SECRC
                | RCTL_BSIZE_8192;

            self.bar0.write_command(REG_R_CTRL, flags);
        };
    }

    pub fn tx_init(&mut self) {
        let guard = allocator::MAPPER.lock();
        let mapper = guard.as_ref().unwrap();

        let raw_tx_addr = self.tx_descs.as_ptr() as u64;
        let phys_tx_addr = mapper.translate_addr(VirtAddr::new(raw_tx_addr)).unwrap();
        let raw_phys_tx = phys_tx_addr.as_u64();

        for i in 0..E1000_NUM_TX_DESC {
            self.tx_descs[i].addr = 0;
            self.tx_descs[i].cmd = 0;
            self.tx_descs[i].status = TSTA_DD;
        }

        unsafe {
            self.bar0
                .write_command(REG_TX_DESC_LO, (raw_phys_tx & 0xFFFFFFFF) as u32);
            self.bar0
                .write_command(REG_TX_DESC_HI, (raw_phys_tx >> 32) as u32);

            let tx_desc_size = size_of::<E1000TxDesc>();
            self.bar0
                .write_command(REG_TX_DESC_LEN, (E1000_NUM_TX_DESC * tx_desc_size) as u32);

            self.bar0.write_command(REG_TX_DESC_HEAD, 0);
            self.bar0.write_command(REG_TX_DESC_TAIL, 0);

            let flags = 0x0
                | TCTL_EN
                | TCTL_PSP
                | (0xF << TCTL_CT_SHIFT)
                | (64 << TCTL_COLD_SHIFT)
                | TCTL_RTLC;
            self.bar0.write_command(REG_T_CTRL, flags);

            // TODO: why?
            self.bar0.write_command(0x0410, 0x0060200A);
        };
    }

    extern "x86-interrupt" fn fire(stack_frame: InterruptStackFrame) {
        todo!()
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
        IDT.lock()[usize::from(interrupt_line) + MIN_INTERRUPT].set_handler_fn(E1000::fire);

        self.enable_interrupts();
        self.rx_init();
        self.tx_init();
        print!("E1000 started!");
    }

    fn get_mac_addr(&self) -> [u8; 6] {
        self.mac
    }

    fn send_packet(&mut self, data: &[u8]) -> Result<(), &'static str> {
        let guard = allocator::MAPPER.lock();
        let mapper = guard.as_ref().unwrap();

        let phys_addr = mapper
            .translate_addr(VirtAddr::new(data.as_ptr() as u64))
            .ok_or("failed to translate TX buffer addr")?;

        let curr_idx = self.tx_cur as usize;
        self.tx_descs[curr_idx].addr = phys_addr.as_u64();
        self.tx_descs[curr_idx].length = data.len() as u16;
        self.tx_descs[curr_idx].cmd = 0x0;
        self.tx_descs[curr_idx].status = CMD_EOP | CMD_IFCS | CMD_RS;

        let old_idx = curr_idx;
        self.tx_cur = (self.tx_cur + 1) % (E1000_NUM_TX_DESC as u16);
        unsafe {
            self.bar0
                .write_command(REG_TX_DESC_TAIL, self.tx_cur as u32)
        };

        while self.tx_descs[old_idx].status & TSTA_DD == 0 {}
        Ok(())
    }
}
