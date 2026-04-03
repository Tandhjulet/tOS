use core::{
    cell::Cell,
    ptr::{read_volatile, write_volatile},
};

use x86_64::{
    PhysAddr, VirtAddr,
    instructions::{interrupts::without_interrupts, port::Port},
};

use crate::{allocator::mmio, pci::PciDevice};

#[derive(Debug, Clone, Copy)]
pub struct IoAddr(pub u16);

#[derive(Debug, Clone, Copy)]
pub enum MemAddr {
    Bits32(PhysAddr),
    Bits64(PhysAddr),
}

impl MemAddr {
    pub fn phys(&self) -> PhysAddr {
        match self {
            MemAddr::Bits32(a) | MemAddr::Bits64(a) => *a,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum BarKind {
    Io { addr: IoAddr },
    Mem { addr: MemAddr, prefetchable: bool },
}

#[derive(Debug)]
pub struct Bar {
    kind: BarKind,
    size: u32,

    offset: u8,

    // Only meaningful for Mem Baars
    virt: Cell<Option<VirtAddr>>,
}

// I/O Space BAR layout
// Bits 31-2						Bit 1		Bit 0
// 4-Byte Aligned Base Address		Reserved	Always 1

// Memory Space BAR layout:
// Bits 31-4						Bit 3			Bits 2-1	Bit 0
// 16-Byte Aligned Base Address		Prefetchable	Type		Always 0
impl Bar {
    pub fn from_cfg(dev: &PciDevice, offset: u8) -> (Self, u8) {
        let raw = dev.read(offset);

        let (kind, slots_used) = Self::parse_kind(&dev, offset, raw);
        let size = Self::probe_size(&dev, offset, raw, &kind);

        (
            Bar {
                kind,
                size,
                offset,
                virt: Cell::new(None),
            },
            slots_used,
        )
    }

    fn parse_kind(dev: &PciDevice, offset: u8, raw: u32) -> (BarKind, u8) {
        if raw & 0x1 != 0 {
            let addr = IoAddr((raw & 0xFFFF_FFFC) as u16);
            (BarKind::Io { addr }, 1)
        } else {
            let mem_type = (raw >> 1) & 0x3;
            let prefetchable = (raw >> 3) & 0x1 != 0;

            match mem_type {
                // 32-bit
                0b00 => {
                    let phys = PhysAddr::new((raw & 0xFFFF_FFF0) as u64);
                    (
                        BarKind::Mem {
                            addr: MemAddr::Bits32(phys),
                            prefetchable,
                        },
                        1,
                    )
                }
                // 64-bit
                0b10 => {
                    let raw_hi = dev.read(offset + 4);
                    let phys = PhysAddr::new(((raw_hi as u64) << 32) | (raw & 0xFFFF_FFF0) as u64);
                    (
                        BarKind::Mem {
                            addr: MemAddr::Bits64(phys),
                            prefetchable,
                        },
                        2,
                    )
                }
                other => panic!("Unknown BAR memory type: {:#b}", other),
            }
        }
    }

    fn probe_size(dev: &PciDevice, offset: u8, original: u32, kind: &BarKind) -> u32 {
        dev.write(offset, 0xFFFF_FFFF);
        let mask = dev.read(offset);
        dev.write(offset, original);

        match kind {
            BarKind::Io { .. } => !(mask & 0xFFFF_FFFC) + 1,
            BarKind::Mem { .. } => !(mask & 0xFFFF_FFF0) + 1,
        }
    }

    pub fn kind(&self) -> BarKind {
        self.kind
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn offset(&self) -> u8 {
        self.offset
    }

    pub fn virt_addr(&self) -> Option<VirtAddr> {
        self.virt.get()
    }

    pub fn set_virt_addr(&self, addr: VirtAddr) {
        assert!(
            matches!(self.kind, BarKind::Mem { .. }),
            "Cannot set virt_addr for an I/O BAR"
        );
        self.virt.set(Some(addr));
    }

    pub fn map_mmio(&self) {
        match self.kind() {
            BarKind::Io { .. } => {}
            BarKind::Mem { addr, .. } => {
                let virt_addr = {
                    let phys_addr = addr.phys();

                    let size = self.size() as u64;
                    mmio::map_mmio(phys_addr, size)
                };

                self.set_virt_addr(virt_addr);
            }
        }
    }

    pub unsafe fn write32(&self, reg_offset: u32, val: u32) {
        match self.kind {
            BarKind::Io { addr } => unsafe {
                let mut port: Port<u32> = Port::new(addr.0 + (reg_offset as u16));
                port.write(val);
            },
            BarKind::Mem { .. } => {
                let virt = self
                    .virt_addr()
                    .expect("write32 on Mem BAR before virt_addr was set");

                let ptr = (virt.as_u64() + reg_offset as u64) as *mut u32;
                without_interrupts(|| unsafe {
                    write_volatile(ptr, val);
                })
            }
        }
    }

    pub unsafe fn read32(&self, reg_offset: u32) -> u32 {
        match self.kind {
            BarKind::Io { addr } => unsafe {
                let mut port: Port<u32> = Port::new(addr.0 + (reg_offset as u16));
                port.read()
            },
            BarKind::Mem { .. } => {
                let virt = self
                    .virt_addr()
                    .expect("read32 on Mem BAR before virt_addr was set");

                let ptr = (virt.as_u64() + reg_offset as u64) as *const u32;
                without_interrupts(|| unsafe { read_volatile(ptr) })
            }
        }
    }

    pub unsafe fn write64(&self, reg_offset: u32, val: u64) {
        unsafe {
            self.write32(reg_offset, val as u32);
            self.write32(reg_offset + 4, (val >> 32) as u32);
        }
    }

    pub unsafe fn read64(&self, reg_offset: u32) -> u64 {
        unsafe {
            let lo = self.read32(reg_offset) as u64;
            let hi = self.read32(reg_offset + 4) as u64;
            (hi << 32) | lo
        }
    }
}
