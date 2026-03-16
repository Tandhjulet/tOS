use core::ptr::{read_volatile, write_volatile};

use x86_64::instructions::port::Port;

use crate::pci::PciDevice;

pub enum AnyBAR<'a> {
    IO(IOBar<'a>),
    Mem(MemBar<'a>),
}

impl<'a> AnyBAR<'a> {
    pub fn size(&self) -> u32 {
        match self {
            AnyBAR::IO(io) => io.size(),
            AnyBAR::Mem(mem) => mem.size(),
        }
    }

    pub fn from_raw(raw: u32, offset: u8, dev: &'a PciDevice) -> Self {
        let is_io: bool = (raw & 0x1) == 0x1;
        if is_io {
            AnyBAR::IO(IOBar::new(raw, offset, dev))
        } else {
            AnyBAR::Mem(MemBar::new(raw, offset, dev))
        }
    }

    pub unsafe fn write_command(&mut self, offset: u16, val: u32) {
        unsafe {
            match self {
                AnyBAR::IO(io) => io.write_command(offset, val),
                AnyBAR::Mem(mem) => mem.write_command(offset, val),
            }
        }
    }

    pub unsafe fn read_command(&mut self, offset: u16) -> u32 {
        unsafe {
            match self {
                AnyBAR::IO(io) => io.read_command(offset),
                AnyBAR::Mem(mem) => mem.read_command(offset),
            }
        }
    }
}

pub trait BAR {
    type AddrType;

    fn device(&self) -> &PciDevice;
    fn raw(&self) -> u32;
    fn offset(&self) -> u8;

    fn is_io(&self) -> bool {
        let is_io: bool = (self.raw() & 0x1) == 0x1;
        is_io
    }

    fn is_mem(&self) -> bool {
        !self.is_io()
    }

    fn addr(&self) -> Self::AddrType;
    fn size(&self) -> u32 {
        let device = self.device();
        let raw = self.raw();
        let offset = self.offset();

        device.write(offset, 0xFFFF_FFFF);
        let mask = device.read(offset);
        device.write(offset, raw);

        if self.is_io() {
            let size = !(mask & 0xFFFF_FFFC) + 1;
            size
        } else {
            let size = !(mask & 0xFFFF_FFF0) + 1;
            size
        }
    }

    unsafe fn write_command(&mut self, offset: u16, val: u32);
    unsafe fn read_command(&mut self, offset: u16) -> u32;
}

pub struct IOBar<'a> {
    raw: u32,
    offset: u8,
    dev: &'a PciDevice,

    base_addr: u16,

    port_base: Port<u32>,
    port_data: Port<u32>,
}

pub struct MemBar<'a> {
    raw: u32,
    dev: &'a PciDevice,
    offset: u8,

    base_addr: u64,
}

impl<'a> MemBar<'a> {
    fn is_32(raw: u32) -> bool {
        let mem_type = (raw >> 1) & 0x3;
        mem_type == 0x0
    }

    fn is_64(raw: u32) -> bool {
        !MemBar::is_32(raw)
    }

    fn addr(raw: u32) -> u64 {
        if MemBar::is_64(raw) {
            panic!("64-bit BARs are unsupported!");
        }

        let bar_low = raw & 0xFFFFFFF0;
        bar_low as u64
    }

    pub fn new(raw: u32, offset: u8, dev: &'a PciDevice) -> Self {
        let base_addr = MemBar::addr(raw);
        MemBar {
            raw,
            dev,
            base_addr,
            offset,
        }
    }
}

// Memory Space BAR layout:
// Bits 31-4						Bit 3			Bits 2-1	Bit 0
// 16-Byte Aligned Base Address		Prefetchable	Type		Always 0
impl<'a> BAR for MemBar<'a> {
    // FIXME: only supporting 32-bit membar addrs
    type AddrType = u32;

    fn addr(&self) -> Self::AddrType {
        if MemBar::is_64(self.raw) {
            panic!("64-bit BARs are unsupported!");
        }

        let bar_low = self.raw & 0xFFFFFFF0;
        bar_low
    }

    unsafe fn write_command(&mut self, offset: u16, val: u32) {
        let addr = self.base_addr + (offset as u64);
        let ptr = addr as *mut u32;
        unsafe { write_volatile(ptr, val) };
    }

    unsafe fn read_command(&mut self, offset: u16) -> u32 {
        let addr = self.base_addr + (offset as u64);
        unsafe {
            return read_volatile(addr as *const u32);
        }
    }

    fn device(&self) -> &PciDevice {
        self.dev
    }

    fn raw(&self) -> u32 {
        self.raw
    }

    fn offset(&self) -> u8 {
        self.offset
    }
}

impl<'a> IOBar<'a> {
    pub fn addr(raw: u32) -> u16 {
        (raw & 0xFFFFFFFC) as u16
    }

    pub fn new(raw: u32, offset: u8, dev: &'a PciDevice) -> Self {
        let base_addr = IOBar::addr(raw);
        IOBar {
            raw,
            dev,
            base_addr,
            port_base: Port::new(base_addr),
            port_data: Port::new(base_addr + 4),
            offset,
        }
    }
}

// I/O Space BAR layout
// Bits 31-2						Bit 1		Bit 0
// 4-Byte Aligned Base Address		Reserved	Always 1
impl<'a> BAR for IOBar<'a> {
    type AddrType = u16;

    fn addr(&self) -> Self::AddrType {
        self.base_addr
    }

    unsafe fn write_command(&mut self, offset: u16, val: u32) {
        unsafe {
            self.port_base.write(offset as u32);
            self.port_data.write(val);
        };
    }

    unsafe fn read_command(&mut self, offset: u16) -> u32 {
        unsafe {
            self.port_base.write(offset as u32);
            self.port_data.read()
        }
    }

    fn device(&self) -> &PciDevice {
        self.dev
    }

    fn raw(&self) -> u32 {
        self.raw
    }

    fn offset(&self) -> u8 {
        self.offset
    }
}
