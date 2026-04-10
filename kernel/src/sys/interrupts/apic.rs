use alloc::vec::Vec;
use x86_64::PhysAddr;

use crate::allocator::mmio::{MappedRegion, map_mmio};

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

pub struct IoApicInfo {
    pub id: u8,
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
            id,
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
            self.write_redirect_entry(idx);
        }
    }

    pub fn write_redirect_entry(&mut self, index: u8) {}
    pub fn read_redirect_entry(&mut self) {}

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
