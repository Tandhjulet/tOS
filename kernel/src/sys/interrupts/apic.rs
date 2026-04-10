use alloc::{format, string::String, vec::Vec};
use log::error;
use x86_64::PhysAddr;

use crate::{
    allocator::mmio::{MappedRegion, map_mmio},
    sys::interrupts::MIN_INTERRUPT,
};

pub struct ApicInfo {
    pub lapic: Lapic,
    pub ioapics: Vec<IoApicInfo>,
    pub iso: Vec<IntSourceOverride>,
    pub cpus: Vec<CpuInfo>,
}

impl ApicInfo {
    pub fn new(lapic_addr: u64) -> Self {
        Self {
            lapic: Lapic::new_mapped(lapic_addr),
            ioapics: Vec::new(),
            iso: Vec::new(),
            cpus: Vec::new(),
        }
    }
}

pub struct Lapic {
    region: MappedRegion,
}

impl Lapic {
    const SVR_OFFSET: usize = 0xF0;
    const TPR_OFFSET: usize = 0x80;

    pub fn new_mapped(lapic_addr: u64) -> Self {
        let region = map_mmio(PhysAddr::new(lapic_addr), 0x1000);
        Self { region }
    }

    pub unsafe fn new(region: MappedRegion) -> Self {
        Self { region }
    }

    pub unsafe fn enable(&self) {
        let svr = unsafe { self.read(Self::SVR_OFFSET) };

        // Bit 8 = APIC Software Enable, 0-7 = spurious vector
        unsafe { self.write(Self::SVR_OFFSET, svr | (1 << 8) | 0xFF) };

        unsafe { self.write(Self::TPR_OFFSET, 0) };
    }

    pub unsafe fn read(&self, offset: usize) -> u32 {
        let ptr = self.region.as_ptr::<u32>();
        unsafe { ptr.byte_add(offset).read_volatile() }
    }

    pub unsafe fn write(&self, offset: usize, value: u32) {
        let ptr = self.region.as_mut_ptr::<u32>();
        unsafe { ptr.byte_add(offset).write_volatile(value) }
    }
}

#[derive(PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum TriggerMode {
    Edge = 0,
    Level = 1,
}

#[derive(PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum PinPolarity {
    ActiveHigh = 0,
    ActiveLow = 1,
}

#[derive(PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum DeliveryMode {
    Fixed = 0b000,
    LowestPriority = 0b001,
    Smi = 0b010,
    Nmi = 0b100,
    Init = 0b101,
    ExtInt = 0b111,
}

#[derive(PartialEq, Eq, Clone, Copy)]
#[repr(u8)]
pub enum DestinationMode {
    Physical = 0,
    Logical = 1,
}

pub struct RedirectionEntry(pub u64);

impl RedirectionEntry {
    pub fn new(
        vector: u8,
        delivery: DeliveryMode,
        dest_mode: DestinationMode,
        pin_polary: PinPolarity,
        trigger_mode: TriggerMode,
        masked: bool,
        dest: u8,
    ) -> Self {
        let mut raw = 0u64;
        raw |= vector as u64;
        raw |= ((delivery as u64) & 0b111) << 8;
        raw |= ((dest_mode as u64) & 0b1) << 11;
        // Bit 12: delivery status
        raw |= ((pin_polary as u64) & 0b1) << 13;
        // Bit 14: remote IRR
        raw |= ((trigger_mode as u64) & 0b1) << 15;
        if masked {
            raw |= 1 << 16;
        }

        let dest_field = if dest_mode == DestinationMode::Physical {
            dest & 0b1111
        } else {
            dest
        };
        raw |= (dest_field as u64) << 56;

        Self(raw as u64)
    }

    pub fn low(&self) -> u32 {
        self.0 as u32
    }

    pub fn high(&self) -> u32 {
        (self.0 >> 32) as u32
    }
}

pub struct IoApicInfo {
    pub apic_id: u8,
    pub region: MappedRegion,
    pub gsi_base: u32,
    pub ver: u8,
    pub redirection_cnt: u8,
    pub nmis: Vec<IoApicNmi>,
}

impl IoApicInfo {
    const IOREGSEL: usize = 0x00;
    const IOREGWIN: usize = 0x10;

    #[allow(unused)]
    const IO_APIC_ID: u8 = 0x00;
    #[allow(unused)]
    const IO_APIC_VER: u8 = 0x01;
    #[allow(unused)]
    const IO_APIC_ARB: u8 = 0x02;

    pub fn new(id: u8, addr: u64, gsi_base: u32) -> Self {
        let region = map_mmio(PhysAddr::new(addr), 0x1000);

        let entry_cnt = unsafe { Self::read(&region, Self::IO_APIC_VER) };
        let ver = entry_cnt as u8;
        let redirection_cnt = (entry_cnt >> 16) as u8 + 1;

        Self {
            apic_id: id,
            region,
            gsi_base,
            redirection_cnt,
            ver,
            nmis: Vec::new(),
        }
    }

    pub unsafe fn init(&mut self, iso: &[IntSourceOverride]) {
        let (gsi_base, gsi_end) = {
            let base = self.gsi_base;
            (base, base + self.redirection_cnt as u32)
        };

        let isa_overrides = iso
            .iter()
            .filter(|o| o.bus == 0)
            .filter(|o| o.gsi >= gsi_base && o.gsi < gsi_end);

        for o in isa_overrides {
            let idx = (o.gsi - gsi_base) as u8;
            if let Err(msg) = self.write_redirect_entry(
                idx,
                RedirectionEntry::new(
                    o.bus_irq + MIN_INTERRUPT as u8,
                    DeliveryMode::Fixed,
                    DestinationMode::Physical,
                    o.pin_polarity(),
                    o.trigger_mode(),
                    true,
                    self.apic_id,
                ),
            ) {
                error!("I/O APIC: {}", msg);
            }
        }
    }

    const fn get_entry_reg(&self, idx: u8) -> u8 {
        0x10 + 2 * idx
    }

    pub fn write_redirect_entry(&self, idx: u8, entry: RedirectionEntry) -> Result<(), String> {
        if idx >= self.redirection_cnt {
            return Err(format!(
                "Tried writing I/O APIC redirection entry @ idx {} despite only having {} capacity",
                idx, self.redirection_cnt
            ));
        }

        let ptr = self.get_entry_reg(idx);
        unsafe {
            Self::write(&self.region, ptr, entry.low());
            Self::write(&self.region, ptr + 1, entry.high());
        };

        Ok(())
    }

    pub fn read_redirect_entry(&self, idx: u8) -> Result<RedirectionEntry, String> {
        if idx >= self.redirection_cnt {
            return Err(format!(
                "Tried writing I/O APIC redirection entry @ idx {} despite only having {} capacity",
                idx, self.redirection_cnt
            ));
        }

        let ptr = self.get_entry_reg(idx);
        let (low, high) = unsafe {
            (
                Self::read(&self.region, ptr),
                Self::read(&self.region, ptr + 1),
            )
        };

        let raw = ((high as u64) << 32) | (low as u64);
        Ok(RedirectionEntry(raw))
    }

    pub unsafe fn read(region: &MappedRegion, reg: u8) -> u32 {
        let ptr = region.as_mut_ptr::<u32>();
        unsafe { ptr.byte_add(Self::IOREGSEL).write_volatile(reg as u32) };

        unsafe { ptr.byte_add(Self::IOREGWIN).read_volatile() }
    }

    pub unsafe fn write(region: &MappedRegion, reg: u8, value: u32) {
        let ptr = region.as_mut_ptr::<u32>();
        unsafe { ptr.byte_add(Self::IOREGSEL).write_volatile(reg as u32) };
        unsafe { ptr.byte_add(Self::IOREGWIN).write_volatile(value as u32) };
    }
}

pub struct IntSourceOverride {
    pub bus: u8,
    pub bus_irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

impl IntSourceOverride {
    pub fn pin_polarity(&self) -> PinPolarity {
        match self.flags & 0b11 {
            0b01 => PinPolarity::ActiveHigh,
            0b11 => PinPolarity::ActiveLow,
            _ => PinPolarity::ActiveHigh,
        }
    }

    pub fn trigger_mode(&self) -> TriggerMode {
        match (self.flags >> 2) & 0b11 {
            0b01 => TriggerMode::Edge,
            0b11 => TriggerMode::Level,
            _ => TriggerMode::Edge,
        }
    }
}

pub struct CpuInfo {
    pub processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
    pub nmis: Vec<LapicNmi>,
}

#[derive(Clone, Copy)]
pub struct LapicNmi {
    pub flags: u16,
    pub lint: u8,
}

#[derive(Clone, Copy)]
pub struct IoApicNmi {
    pub nmi_src: u8,
    _reserved: u8,
    pub flags: u16,
    pub gsi: u32,
}
