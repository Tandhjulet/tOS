use core::fmt;

use alloc::{borrow::ToOwned, format, string::String, sync::Arc, vec::Vec};
use lazy_static::lazy_static;
use log::error;
use num_enum::TryFromPrimitive;
use spin::Mutex;
use x86_64::PhysAddr;

use crate::{
    allocator::mmio::{MappedRegion, map_mmio},
    io::pci::{
        bar::Bar,
        enumerator::{IoPci, MmioPci, PciEnumerator},
    },
    sys::acpi::{
        ACPI,
        sdt::mcfg::{Mcfg, McfgEntry},
    },
};

pub mod bar;
pub mod enumerator;

lazy_static! {
    pub static ref DEVICES: Mutex<Vec<Arc<Mutex<PciDevice>>>> = Mutex::new(Vec::new());
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

const PCIE_CONFIG_SPACE: usize = 4096;

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

    // PCIe devices have extended configuration space
    ext_cfg: Option<MappedRegion>,
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

#[repr(u8)]
pub enum PciCapability {
    Msi = 0x05,
    MsiX = 0x11,
}

impl PciDevice {
    const OFF_ID: u8 = 0x0;
    const OFF_COMMAND_STATUS: u8 = 0x04;
    const OFF_CLASS: u8 = 0x08;
    const OFF_HEADER: u8 = 0x0C;
    const OFF_INTERRUPT: u8 = 0x3C;
    const OFF_BARS_START: u8 = 0x10;
    const OFF_SEC_BUS: u8 = 0x19;
    const OFF_CAPABILITIES: u8 = 0x34;

    pub fn new_pci(id: u32, bus: u8, device: u8, function: u8) -> Result<Self, String> {
        let vendor_id = (id & 0xFFFF) as u16;
        let device_id = (id >> 16) as u16;

        let class_reg = IoPci::read(bus, device, function, Self::OFF_CLASS);
        let class = (class_reg >> 24) as u8;
        let subclass = (class_reg >> 16) as u8;

        let header_raw = ((IoPci::read(bus, device, function, Self::OFF_CLASS) >> 16) as u8) & 0x7;
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
            ext_cfg: None,
        };
        device.enumerate_bars();

        Ok(device)
    }

    pub fn new_pcie(
        id: u32,
        bus: u8,
        device: u8,
        function: u8,
        entry: McfgEntry,
    ) -> Result<Self, String> {
        let mut dev = Self::new_pci(id, bus, device, function)?;
        dev.set_cfg(entry);
        Ok(dev)
    }

    pub fn read(&self, offset: u8) -> u32 {
        IoPci::read(self.bus, self.device, self.function, offset)
    }

    pub fn write(&self, offset: u8, value: u32) {
        IoPci::write(self.bus, self.device, self.function, offset, value);
    }

    pub fn has_capabilities(&self) -> bool {
        self.status() & (1 << 4) > 0
    }

    pub fn find_capability(&self, to_find: PciCapability) -> Option<u8> {
        if !self.has_capabilities() {
            return None;
        }

        let to_find = to_find as u8;

        let capabilities = self.read(Self::OFF_CAPABILITIES) as u8;
        // mask bottom 2 bits of cap. ptr. to find addr of first cap.
        let mut cap_addr = capabilities & 0xFC;

        // last cap has ptr set to 0
        const FINAL_CAP_ADDR: u8 = 0;
        while cap_addr != FINAL_CAP_ADDR {
            let cap_dword = self.read(cap_addr) as u16;
            let cap_id = cap_dword as u8;
            if cap_id == to_find {
                return Some(cap_addr);
            }

            let next_cap_ptr = (cap_dword >> 8) as u8;
            cap_addr = next_cap_ptr;
        }

        None
    }

    pub fn interrupt_support(&self) -> InterruptSupport {
        InterruptSupport {
            isa: true,
            msi: self.find_capability(PciCapability::Msi).is_some(),
            msix: self.find_capability(PciCapability::MsiX).is_some(),
        }
    }

    pub fn is_pcie(&self) -> bool {
        self.ext_cfg.is_some() // check capabilities too?
    }

    pub fn set_cfg(&mut self, entry: McfgEntry) {
        let phys_cfg_addr = entry.base_addr + (((self.bus - entry.bus_num_start) as u64) << 20)
            | ((self.device as u64) << 15)
            | ((self.function as u64) << 12);

        self.ext_cfg = Some(map_mmio(
            PhysAddr::new(phys_cfg_addr),
            PCIE_CONFIG_SPACE as u64,
        ));
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

    pub fn bar_offset_from_idx(idx: usize) -> u8 {
        Self::OFF_BARS_START + (idx as u8) * 4
    }

    pub fn bar_idx_from_offset(offset: u16) -> usize {
        (offset >> 2) as usize - (Self::OFF_BARS_START >> 2) as usize
    }

    fn enumerate_bars(&mut self) {
        let mut slot = 0;
        while slot < self.header_type.bar_count() {
            let offset = Self::bar_offset_from_idx(slot);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptSupport {
    pub isa: bool,
    pub msi: bool,
    pub msix: bool,
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

pub fn init() {
    if let Err(msg) = init_pci() {
        error!("PCI: {}", msg);
    }

    if let Err(msg) = init_pcie() {
        error!("PCIe: {}", msg);
    }
}

fn init_pci() -> Result<(), String> {
    let found = IoPci.enumerate();

    let mut devices = DEVICES.lock();
    devices.extend(found.into_iter().map(|dev| Arc::new(Mutex::new(dev))));

    Ok(())
}

fn init_pcie() -> Result<(), String> {
    let tables = ACPI
        .get()
        .expect("Cannot init PCIe without ACPI loaded!")
        .tables();

    let raw_mcfg = tables
        .find_table::<Mcfg>()
        .ok_or("Failed finding MCFG table!".to_owned())?;

    let mut devices = DEVICES.lock();
    let mcfg = unsafe { &*raw_mcfg.as_ptr() };
    for entry in mcfg.entries().iter() {
        let enumerator = MmioPci::new(entry);
        let found = enumerator.enumerate();

        for new_dev in found {
            let existing = devices.iter_mut().find(|d| {
                let d = d.lock();
                d.bus == new_dev.bus && d.device == new_dev.device && d.function == new_dev.function
            });

            if let Some(dev) = existing {
                *dev.lock() = new_dev;
            } else {
                devices.push(Arc::new(Mutex::new(new_dev)));
            }
        }
    }

    Ok(())
}
