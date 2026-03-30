use core::ptr::read_volatile;

use alloc::sync::Arc;
use spin::Mutex;
use x86_64::{
    VirtAddr, instructions::interrupts::without_interrupts, structures::idt::InterruptStackFrame,
};

/**
 * See https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf#page389
 * or (https://wiki.osdev.org/Intel_Ethernet_i217) for documentation
 * details regarding the implementation of the E1000 driver.
 */
use crate::{
    allocator::mmio::{self, alloc_dma_region},
    helpers,
    interrupts::{IDT, MIN_INTERRUPT, PICS},
    networking::{MacAddr, RX_QUEUE, drivers::NetworkDriver},
    pci::{
        PciDevice,
        bar::{AnyBAR, BAR},
    },
    println,
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
const REG_I_MASK_CLEAR: u16 = 0x00D8;
const REG_I_READ: u16 = 0xC0;

const I_RXT0: u32 = 0x80; // Receive timer
const I_RX0: u32 = 0x10; // Receive overrun
const I_LSC: u32 = 0x04; // Link Status Change

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

pub struct E1000 {
    interrupt_line: u8,
    device: Arc<Mutex<PciDevice>>,
    bar0: AnyBAR,
    eeprom_exists: bool,
    mac: MacAddr,
    rx_descs_addr: VirtAddr,
    tx_descs_addr: VirtAddr,
    rx_cur: u16,
    tx_cur: u16,
}

impl E1000 {
    pub fn new(guard: Arc<Mutex<PciDevice>>) -> Self {
        let mut bar0 = PciDevice::get_bar(&guard, 0);
        if let AnyBAR::Mem(ref mut bar) = bar0 {
            let virt_addr = {
                let phys_addr = bar.addr();

                let size = bar.size() as u64;
                mmio::map_mmio(phys_addr, size)
            };

            bar.set_virt_addr(virt_addr);
        }

        let irq = {
            let mut device = guard.lock();
            device.enable_bus_mastering();
            device.interrupt_line
        };

        Self {
            device: guard,
            bar0,
            eeprom_exists: false,
            mac: MacAddr::zero(),
            tx_descs_addr: VirtAddr::zero(),
            rx_descs_addr: VirtAddr::zero(),
            rx_cur: 0,
            tx_cur: 0,
            interrupt_line: irq,
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
            let mut mac: [u8; 6] = [0; 6];
            mac[0] = (tmp & 0xff) as u8;
            mac[1] = (tmp >> 8) as u8;

            tmp = self.read_eeprom(1);
            mac[2] = (tmp & 0xff) as u8;
            mac[3] = (tmp >> 8) as u8;

            tmp = self.read_eeprom(2);
            mac[4] = (tmp & 0xff) as u8;
            mac[5] = (tmp >> 8) as u8;
            self.mac = MacAddr { raw: mac };
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

                    let mut mac: [u8; 6] = [0; 6];
                    for i in 0..6 {
                        mac[i] = *mem_base_mac8.add(i);
                    }
                    self.mac = MacAddr { raw: mac };
                }

                return true;
            }
        }
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
            // read to clear all reg bits
            self.bar0.read_command(REG_I_READ);

            self.bar0.write_command(REG_I_MASK_CLEAR, 0xFFFFFFFF);
            let imask = I_RX0 | I_LSC | I_RXT0;
            self.bar0.write_command(REG_I_MASK, imask);
        }
    }

    fn get_desc_buffer_ptr(&self, buffer_idx: usize) -> *const u8 {
        let descs_addr = self.rx_descs_addr.as_u64();
        let buffers_start_addr = descs_addr + (size_of::<E1000RxDesc>() * E1000_NUM_RX_DESC) as u64;

        (buffers_start_addr + (buffer_idx * RX_BUFFER_SIZE) as u64) as *const _
    }

    fn handle_receive(&mut self) -> bool {
        let mut received: bool = false;
        loop {
            let curr_idx = self.rx_cur as usize;
            let desc =
                unsafe { &mut *self.rx_descs_addr.as_mut_ptr::<E1000RxDesc>().add(curr_idx) };

            if unsafe { read_volatile(&desc.status) } & TSTA_DD == 0 {
                break;
            }

            received = true;

            let len = desc.length;
            let packet = unsafe {
                core::slice::from_raw_parts(self.get_desc_buffer_ptr(curr_idx), len as usize)
            };

            RX_QUEUE.lock().push_back(packet.to_vec());

            desc.status = 0;

            let old_cur = self.rx_cur;
            self.rx_cur = (self.rx_cur + 1) % (E1000_NUM_RX_DESC as u16);

            unsafe {
                self.bar0.write_command(REG_RX_DESC_TAIL, old_cur as u32);
            };
        }

        received
    }

    pub fn rx_init(&mut self) {
        const BLOCK_SIZE: usize = size_of::<E1000RxDesc>() * E1000_NUM_RX_DESC;
        const TOTAL_SIZE: u64 = (BLOCK_SIZE + RX_BUFFER_SIZE * E1000_NUM_RX_DESC) as u64;

        let (rx_virt, rx_phys) = alloc_dma_region(TOTAL_SIZE);

        let descs = rx_virt.as_mut_ptr::<E1000RxDesc>();
        let buffers_phys = rx_phys.as_u64() + BLOCK_SIZE as u64;

        for i in 0..E1000_NUM_RX_DESC {
            let buf_phys = buffers_phys + (i * RX_BUFFER_SIZE) as u64;
            unsafe {
                let desc = &mut *descs.add(i);
                desc.addr = buf_phys;
                desc.status = 0;
            }
        }

        self.rx_descs_addr = rx_virt;

        unsafe {
            self.bar0
                .write_command(REG_RX_DESC_LO, (rx_phys.as_u64() & 0xFFFF_FFFF) as u32);
            self.bar0
                .write_command(REG_RX_DESC_HI, (rx_phys.as_u64() >> 32) as u32);
            self.bar0
                .write_command(REG_RX_DESC_LEN, (BLOCK_SIZE) as u32);

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
        let (tx_virt, tx_phys) =
            alloc_dma_region((size_of::<E1000TxDesc>() * E1000_NUM_TX_DESC) as u64);

        let tx_descs = tx_virt.as_mut_ptr::<E1000TxDesc>();
        for i in 0..E1000_NUM_TX_DESC {
            unsafe {
                let desc = tx_descs.add(i);
                (*desc).addr = 0;
                (*desc).cmd = 0;
                (*desc).status = TSTA_DD;
            }
        }

        self.tx_descs_addr = tx_virt;

        unsafe {
            self.bar0
                .write_command(REG_TX_DESC_LO, (tx_phys.as_u64() & 0xFFFFFFFF) as u32);
            self.bar0
                .write_command(REG_TX_DESC_HI, (tx_phys.as_u64() >> 32) as u32);

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
}

impl NetworkDriver for E1000 {
    fn start(&mut self) {
        self.detect_eeprom();
        if !self.read_mac() {
            return;
        }

        self.clear_mta();

        self.rx_init();
        self.tx_init();

        without_interrupts(|| {
            IDT.lock()[(self.interrupt_line as usize) + MIN_INTERRUPT]
                .set_handler_fn(<dyn NetworkDriver>::fire);

            let mut pics = PICS.lock();
            unsafe {
                let [master, slave] = pics.read_masks();
                if self.interrupt_line < 8 {
                    pics.write_masks(master & !(1u8 << self.interrupt_line), slave);
                } else {
                    pics.write_masks(master, slave & !(1u8 << (self.interrupt_line - 8)));
                }
            };
        });

        self.enable_interrupts();

        println!("E1000 started!");
    }

    fn get_mac_addr(&self) -> &MacAddr {
        &self.mac
    }

    fn prepare_transmit(&mut self, data: &[u8]) {
        // TODO: don't realloc to ensure DMA - use pre-alloc buffers for performance
        let (tx_buf_virt, tx_buf_phys) = alloc_dma_region(data.len() as u64);
        unsafe {
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                tx_buf_virt.as_mut_ptr::<u8>(),
                data.len(),
            );
        }

        let curr_idx = self.tx_cur as usize;
        let desc = unsafe { &mut *self.tx_descs_addr.as_mut_ptr::<E1000TxDesc>().add(curr_idx) };

        desc.addr = tx_buf_phys.as_u64();
        desc.length = data.len() as u16;
        desc.cmd = CMD_EOP | CMD_IFCS | CMD_RS;
        desc.status = 0x0;

        self.tx_cur = (self.tx_cur + 1) % (E1000_NUM_TX_DESC as u16);
    }

    fn transmit(&mut self) {
        unsafe {
            self.bar0
                .write_command(REG_TX_DESC_TAIL, self.tx_cur as u32)
        };
    }

    fn is_up(&mut self) -> bool {
        let status = unsafe { self.bar0.read_command(REG_STATUS) };
        status & 0x2 != 0
    }

    fn handle_interrupt(&mut self, _: InterruptStackFrame) {
        let status = unsafe { self.bar0.read_command(REG_I_READ) };
        if status & I_RXT0 > 0 {
            self.handle_receive();
        }

        let imask = I_RX0 | I_LSC | I_RXT0;
        unsafe { self.bar0.write_command(REG_I_MASK, imask) };
    }

    fn get_interrupt_line(&self) -> u8 {
        self.interrupt_line
    }
}
