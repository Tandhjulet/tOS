use core::{
    fmt,
    ops::{Deref, DerefMut},
    slice,
};

use alloc::{borrow::ToOwned, format, string::String, sync::Arc, vec::Vec};
use lazy_static::lazy_static;
use log::error;
use num_enum::TryFromPrimitive;
use spin::Mutex;
use volatile::Volatile;
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

    ///
    /// ### Arguments
    /// `lapic_id`: core that the interrupt will be routed to
    /// `int_num`: interrupt vector to assign
    ///
    pub fn enable_msi(&self, lapic_id: u8, int_num: u8) -> Result<(), &'static str> {
        let cap_addr = self
            .find_capability(PciCapability::Msi)
            .ok_or("PCI device does not support MSI")?;

        let msi_reg_index = cap_addr >> 2;

        // Message Address Register
        const MSG_ADDR_REG_OFFSET: u8 = 1;
        const MEMORY_REGION: u32 = 0x0FEE << 20;
        let core = (lapic_id as u32) << 12;
        self.write(msi_reg_index + MSG_ADDR_REG_OFFSET, MEMORY_REGION | core);

        // Message Data Register
        let mut header = self.read(msi_reg_index);
        let is_64bit = header >> (16 + 7) & 1 > 0;
        let msg_data_offset: u8 = if is_64bit { 3 } else { 2 };
        self.write(msi_reg_index + msg_data_offset, int_num as u32);

        // Enable MSI (bit 16 of header)
        const MSI_ENABLE: u32 = 1 << 16;
        header |= MSI_ENABLE;
        self.write(msi_reg_index, header);

        Ok(())
    }

    pub fn enable_msix(&self) -> Result<(), &'static str> {
        let cap_addr = self
            .find_capability(PciCapability::MsiX)
            .ok_or("PCI device does not support MSI-X")?;

        // Enable MSIX (bit 15 of upper half-dword)
        let mut header = self.read(cap_addr);
        const MSIX_ENABLE: u32 = 1 << (16 + 15);
        header |= MSIX_ENABLE;
        self.write(cap_addr, header);

        Ok(())
    }

    pub fn get_msix_tables(&self) -> Result<MsixVectorTable<'_>, &'static str> {
        let cap_addr = self
            .find_capability(PciCapability::MsiX)
            .ok_or("PCI device does not support MSI-X")?;

        const VECTOR_TABLE_OFF: u8 = 4;
        let table = self.read(cap_addr + VECTOR_TABLE_OFF);
        let bir = table & 0x7;
        let Some(bar) = self.get_bar(bir as usize) else {
            Err("Failed finding MSI-X bar for PCI device!")?
        };

        let ctrl = self.read(cap_addr) >> 16;
        let table_size = ctrl & 0x3FF;
        let table_offset = table >> 3;

        bar.map_mmio();

        let vector_table = MsixVectorTable::new(bar, table_size as usize, table_offset as usize);
        Ok(vector_table)
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

pub struct MsixVectorTable<'a> {
    entries: &'a mut [MsixVectorEntry],
}

impl<'a> MsixVectorTable<'a> {
    pub fn new(bar: &'a Bar, table_entries: usize, table_offset: usize) -> Self {
        bar.map_mmio();
        let region = bar.region().unwrap();
        let raw_ptr = unsafe {
            region
                .as_mut_ptr::<MsixVectorEntry>()
                .byte_add(table_offset)
        };

        let slice = unsafe { slice::from_raw_parts_mut(raw_ptr, table_entries) };
        Self { entries: slice }
    }
}

impl Deref for MsixVectorTable<'_> {
    type Target = [MsixVectorEntry];

    fn deref(&self) -> &Self::Target {
        self.entries
    }
}

impl DerefMut for MsixVectorTable<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.entries
    }
}

#[repr(C)]
pub struct MsixVectorEntry {
    msg_low_addr: Volatile<u32>,
    msg_high_addr: Volatile<u32>,
    msg_data: Volatile<u32>,
    vector_ctrl: Volatile<u32>,
}

impl MsixVectorEntry {
    const UNMASK_INT: u32 = 0;
    const DEST_ID_SHIFT: u32 = 12;
    const RSVD_INTR_REG: u32 = 0xFEE << 20;

    pub fn init(&mut self, cpu_id: u8, int_num: u32) {
        let dest_id = (cpu_id as u32) << Self::DEST_ID_SHIFT;
        self.msg_low_addr.write(Self::RSVD_INTR_REG | dest_id);
        self.msg_high_addr.write(0);
        self.msg_data.write(int_num);
        self.vector_ctrl.write(Self::UNMASK_INT);
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
