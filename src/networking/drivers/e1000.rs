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
    interrupts::{IDT, MIN_INTERRUPT, PICS},
    networking::{MacAddr, RX_QUEUE, drivers::NetworkDriver},
    pci::{
        PciDevice,
        bar::{AnyBAR, BAR},
    },
    println,
};

#[allow(unused)]
pub(super) mod cfg {
    use crate::helpers;

    // Refer to Table 13-2 in https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf#page389
    // for documentation about these constants
    pub const CTRL: u16 = 0x0; // Device Control
    pub const STATUS: u16 = 0x0008; // Device Status
    pub const TXCW: u16 = 0x0178; // Transmit Configuration
    pub const EEPROM: u16 = 0x0014; // EEPROM Read

    pub const NUM_RX_DESC: usize = 32;
    pub const NUM_TX_DESC: usize = 8;

    pub mod rx {
        pub const CTRL: u16 = 0x0100; // Receive Control

        pub const DESC_LO: u16 = 0x2800; // Receive Descriptor Base Low
        pub const DESC_HI: u16 = 0x2804; // Receive Descriptor Base High

        pub const DESC_LEN: u16 = 0x2808;
        pub const DESC_HEAD: u16 = 0x2810;
        pub const DESC_TAIL: u16 = 0x2818;

        pub mod ctrl {
            //
            // REG_R_CTRL (Receive Control) FLAGS (see 13.4.22 intel docs)
            //
            pub const EN: u32 = 1 << 1; // Enable Receiver
            pub const SBP: u32 = 1 << 2; // Store Bad Packets
            pub const UPE: u32 = 1 << 3; // Unicast Promiscuous Enabled
            pub const MPE: u32 = 1 << 4; // Multicast Promiscuous Enabled
            pub const LPE: u32 = 1 << 5; // Long Packet Reception Enable
            // Loopback Mode
            pub const LBM_NONE: u32 = 0 << 6; // No Loopback
            pub const LBM_PHY: u32 = 0b11 << 6; // PHY or external SerDesc loopback
            // Receive Descriptor Minimum Threshold Size
            pub const RDMTS_HALF: u32 = 0 << 8; // Threshold is 1/2 of total RX circular desc buffer
            pub const RDMTS_QUARTER: u32 = 1 << 8; // Threshold is 1/4 of total RX circular desc buffer
            pub const RDMTS_EIGHT: u32 = 2 << 8; // Threshold is 1/8 of total RX circular desc buffer
            // Multicast Offset
            pub const MO_36: u32 = 0 << 12; // Use bits 47:36
            pub const MO_35: u32 = 1 << 12; // Use bits 47:35
            pub const MO_34: u32 = 2 << 12; // Use bits 47:34
            pub const MO_32: u32 = 3 << 12; // Use bits 47:32
            pub const BAM: u32 = 1 << 15; // Broadcast Accept Mode
            // Buffer sizes
            pub const BSIZE_256: u32 = 3 << 16;
            pub const BSIZE_512: u32 = 2 << 16;
            pub const BSIZE_1024: u32 = 1 << 16;
            pub const BSIZE_2048: u32 = 0 << 16;
            pub const BSIZE_4096: u32 = (3 << 16) | (1 << 25);
            pub const BSIZE_8192: u32 = (2 << 16) | (1 << 25);
            pub const BSIZE_16384: u32 = (1 << 16) | (1 << 25);
            pub const VFE: u32 = 1 << 18; // VLAN Filter Enable
            pub const CFIEN: u32 = 1 << 19; // Canonical Form Indicator Enable
            pub const CFI: u32 = 1 << 20; // Canonical Form Indicator Bit Value
            pub const DPF: u32 = 1 << 22; // Discard Pause Frames
            pub const PMCF: u32 = 1 << 23; // Pass MAC Control Frames
            pub const SECRC: u32 = 1 << 26; // Strip Ethernet CRC
        }
    }

    pub mod tx {
        pub const CTRL: u16 = 0x0400; // Transmit Control
        pub const DESC_LO: u16 = 0x3800; // Transmit Descriptor Base Low
        pub const DESC_HI: u16 = 0x3804; // Transmit Descriptor Base High

        pub const DESC_LEN: u16 = 0x3808;
        pub const DESC_HEAD: u16 = 0x3810;
        pub const DESC_TAIL: u16 = 0x3818;

        pub mod ctrl {
            //
            // TCTL (Transmit Control) FLAGS (See 13.4.33 Intel docs)
            //
            pub const EN: u32 = 0x1 << 1; // Transmit Enable
            pub const PSP: u32 = 0x1 << 3; // Pad Short Packets
            pub const CT_SHIFT: u32 = 4; // Collision Threshold
            pub const COLD_SHIFT: u32 = 12; // Collision Distance
            pub const SWXOFF: u32 = 1 << 22; // Software XOFF Transmission
            pub const RTLC: u32 = 1 << 24; // Re-transmit on Late Collision
        }

        pub const STATUS_DD: u8 = 1 << 0; // Descriptor Done
        pub const STATUS_EC: u8 = 1 << 1; // Express Collisions
        pub const STATUS_LC: u8 = 1 << 2; // Late Collision
        pub const STATUS_TU: u8 = 1 << 3; // Transmit Underrun

        pub const CMD_EOP: u8 = 1 << 0;
        pub const CMD_IFCS: u8 = 1 << 1;
        pub const CMD_RS: u8 = 1 << 3;
    }

    pub mod int {
        // INTERRUPTS
        pub const MASK: u16 = 0x00D0; // Interrupt Mask Set/Read
        pub const MASK_CLEAR: u16 = 0x00D8;
        pub const CAUSE_READ: u16 = 0xC0;

        pub const ICR_RXT0: u32 = 0x80; // Receive timer
        pub const ICR_RX0: u32 = 0x10; // Receive overrun
        pub const ICR_LSC: u32 = 0x04; // Link Status Change
    }

    // MISC
    pub const MTA_LENGTH: usize = 128;
    pub const REG_MTA: [u16; MTA_LENGTH] = helpers::build_range::<MTA_LENGTH>(0x5200, 4); // Multicast Table Array 

    pub const RX_BUFFER_SIZE: usize = 8192;
}

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
        unsafe { self.bar0.write_command(cfg::EEPROM, 0x1) };

        self.eeprom_exists = false;
        for _ in 0..1000 {
            if unsafe { self.bar0.read_command(cfg::EEPROM) } & 0x10 > 0 {
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
                    .write_command(cfg::EEPROM, 1 | ((addr as u32) << 8))
            };
            while {
                tmp = unsafe { self.bar0.read_command(cfg::EEPROM) };
                tmp & (1 << 4) == 0
            } {}
        } else {
            unsafe {
                self.bar0
                    .write_command(cfg::EEPROM, 1 | ((addr as u32) << 2))
            };
            while {
                tmp = unsafe { self.bar0.read_command(cfg::EEPROM) };
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
        for addr in cfg::REG_MTA {
            unsafe {
                self.bar0.write_command(addr, 0x0);
            };
        }
    }

    pub fn enable_interrupts(&mut self) {
        unsafe {
            // read to clear all reg bits
            self.bar0.read_command(cfg::int::CAUSE_READ);

            self.bar0.write_command(cfg::int::MASK_CLEAR, 0xFFFFFFFF);

            let imask = cfg::int::ICR_RX0 | cfg::int::ICR_LSC | cfg::int::ICR_RXT0;
            self.bar0.write_command(cfg::int::MASK, imask);
        }
    }

    fn get_desc_buffer_ptr(&self, buffer_idx: usize) -> *const u8 {
        let descs_addr = self.rx_descs_addr.as_u64();
        let buffers_start_addr = descs_addr + (size_of::<E1000RxDesc>() * cfg::NUM_RX_DESC) as u64;

        (buffers_start_addr + (buffer_idx * cfg::RX_BUFFER_SIZE) as u64) as *const _
    }

    fn handle_receive(&mut self) -> bool {
        let mut received: bool = false;
        loop {
            let curr_idx = self.rx_cur as usize;
            let desc =
                unsafe { &mut *self.rx_descs_addr.as_mut_ptr::<E1000RxDesc>().add(curr_idx) };

            if unsafe { read_volatile(&desc.status) } & cfg::tx::STATUS_DD == 0 {
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
            self.rx_cur = (self.rx_cur + 1) % (cfg::NUM_RX_DESC as u16);

            unsafe {
                self.bar0.write_command(cfg::rx::DESC_TAIL, old_cur as u32);
            };
        }

        received
    }

    pub fn rx_init(&mut self) {
        const BLOCK_SIZE: usize = size_of::<E1000RxDesc>() * cfg::NUM_RX_DESC;
        const TOTAL_SIZE: u64 = (BLOCK_SIZE + cfg::RX_BUFFER_SIZE * cfg::NUM_RX_DESC) as u64;

        let (rx_virt, rx_phys) = alloc_dma_region(TOTAL_SIZE);

        let descs = rx_virt.as_mut_ptr::<E1000RxDesc>();
        let buffers_phys = rx_phys.as_u64() + BLOCK_SIZE as u64;

        for i in 0..cfg::NUM_RX_DESC {
            let buf_phys = buffers_phys + (i * cfg::RX_BUFFER_SIZE) as u64;
            unsafe {
                let desc = &mut *descs.add(i);
                desc.addr = buf_phys;
                desc.status = 0;
            }
        }

        self.rx_descs_addr = rx_virt;

        unsafe {
            self.bar0
                .write_command(cfg::rx::DESC_LO, (rx_phys.as_u64() & 0xFFFF_FFFF) as u32);
            self.bar0
                .write_command(cfg::rx::DESC_HI, (rx_phys.as_u64() >> 32) as u32);
            self.bar0
                .write_command(cfg::rx::DESC_LEN, (BLOCK_SIZE) as u32);

            self.bar0.write_command(cfg::rx::DESC_HEAD, 0);
            self.bar0
                .write_command(cfg::rx::DESC_TAIL, (cfg::NUM_RX_DESC - 1) as u32);

            let flags = 0x0
                | cfg::rx::ctrl::EN
                | cfg::rx::ctrl::SBP
                | cfg::rx::ctrl::UPE
                | cfg::rx::ctrl::MPE
                | cfg::rx::ctrl::LBM_NONE
                | cfg::rx::ctrl::RDMTS_HALF
                | cfg::rx::ctrl::BAM
                | cfg::rx::ctrl::SECRC
                | cfg::rx::ctrl::BSIZE_8192;

            self.bar0.write_command(cfg::rx::CTRL, flags);
        };
    }

    pub fn tx_init(&mut self) {
        let (tx_virt, tx_phys) =
            alloc_dma_region((size_of::<E1000TxDesc>() * cfg::NUM_TX_DESC) as u64);

        let tx_descs = tx_virt.as_mut_ptr::<E1000TxDesc>();
        for i in 0..cfg::NUM_TX_DESC {
            unsafe {
                let desc = tx_descs.add(i);
                (*desc).addr = 0;
                (*desc).cmd = 0;
                (*desc).status = cfg::tx::STATUS_DD;
            }
        }

        self.tx_descs_addr = tx_virt;

        unsafe {
            self.bar0
                .write_command(cfg::tx::DESC_LO, (tx_phys.as_u64() & 0xFFFFFFFF) as u32);
            self.bar0
                .write_command(cfg::tx::DESC_HI, (tx_phys.as_u64() >> 32) as u32);

            let tx_desc_size = size_of::<E1000TxDesc>();
            self.bar0
                .write_command(cfg::tx::DESC_LEN, (cfg::NUM_TX_DESC * tx_desc_size) as u32);

            self.bar0.write_command(cfg::tx::DESC_HEAD, 0);
            self.bar0.write_command(cfg::tx::DESC_TAIL, 0);

            let flags = 0x0
                | cfg::tx::ctrl::EN
                | cfg::tx::ctrl::PSP
                | (0xF << cfg::tx::ctrl::CT_SHIFT)
                | (64 << cfg::tx::ctrl::COLD_SHIFT)
                | cfg::tx::ctrl::RTLC;
            self.bar0.write_command(cfg::tx::CTRL, flags);

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
        desc.cmd = cfg::tx::CMD_EOP | cfg::tx::CMD_IFCS | cfg::tx::CMD_RS;
        desc.status = 0x0;

        self.tx_cur = (self.tx_cur + 1) % (cfg::NUM_TX_DESC as u16);
    }

    fn transmit(&mut self) {
        unsafe {
            self.bar0
                .write_command(cfg::rx::DESC_TAIL, self.tx_cur as u32)
        };
    }

    fn is_up(&mut self) -> bool {
        let status = unsafe { self.bar0.read_command(cfg::STATUS) };
        status & 0x2 != 0
    }

    fn handle_interrupt(&mut self, _: InterruptStackFrame) {
        let status = unsafe { self.bar0.read_command(cfg::int::CAUSE_READ) };
        if status & cfg::int::ICR_RXT0 > 0 {
            self.handle_receive();
        }

        let imask = cfg::int::ICR_RX0 | cfg::int::ICR_LSC | cfg::int::ICR_RXT0;
        unsafe { self.bar0.write_command(cfg::int::MASK, imask) };
    }

    fn get_interrupt_line(&self) -> u8 {
        self.interrupt_line
    }
}
