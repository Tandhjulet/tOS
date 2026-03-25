pub mod drivers;
pub mod protocols;

use core::fmt::Display;

use alloc::{boxed::Box, sync::Arc, vec};
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{interrupts::PICS, networking::drivers::E1000, println};

pub static NETWORK_DRIVER: Mutex<Option<Box<dyn NetworkDriver>>> = Mutex::new(None);

pub fn init() {
    let mut lock = {
        let mut devices = crate::pci::DEVICES.lock();

        // https://wiki.osdev.org/PCI#Class_Codes
        let device = devices.iter_mut().find(|d| {
            let d = d.lock();
            d.class == 0x2 && d.subclass == 0x0
        });

        let Some(device) = device else { return };

        let device: Arc<Mutex<crate::pci::PciDevice>> = Arc::clone(device);
        drop(devices); // release the DEVICES lock before locking the individual device to avoid potential deadlocks

        let driver = E1000::new(device);
        *NETWORK_DRIVER.lock() = Some(Box::new(driver));

        NETWORK_DRIVER.lock()
    };

    let driver = lock.as_mut().unwrap();
    driver.start();

    println!("driver is up?: {}", driver.is_up());
}

pub trait NetworkDriver: Send {
    fn start(&mut self);
    fn get_mac_addr(&self) -> &MacAddr;
    fn is_up(&mut self) -> bool;
    fn send_raw_data(&mut self, data: &[u8]) -> Result<(), &'static str>;
    fn handle_interrupt(&mut self, stack_frame: InterruptStackFrame);
    fn get_interrupt_line(&self) -> u8;
}

impl dyn NetworkDriver {
    extern "x86-interrupt" fn fire(stack_frame: InterruptStackFrame) {
        let irq_line = {
            let mut driver = NETWORK_DRIVER.lock();
            let driver = driver.as_mut().unwrap();

            driver.handle_interrupt(stack_frame);
            driver.get_interrupt_line()
        };

        unsafe {
            PICS.lock().notify_end_of_interrupt(irq_line);
        }
    }

    // FIXME: this requires a lock to run which is inefficient and unnecessary.
    pub fn send_packet(
        &mut self,
        dst_mac: MacAddr,
        ethertype: EtherType,
        payload: &[u8],
    ) -> Result<(), &'static str> {
        // FIXME: dynamically allocate multiple ethernet frames
        const MAX_PAYLOAD_SIZE: usize = 1500 /* bytes */;
        if payload.len() > MAX_PAYLOAD_SIZE {
            panic!("Payload is too large!");
        }

        let frame = EthernetFrame {
            dst_mac,
            src_mac: *self.get_mac_addr(),
            ethertype,
            payload,
        };

        const FRAME_LEN: usize = EthernetFrame::header_len();
        let mut frame_buf = vec![0; FRAME_LEN + payload.len()];
        let len = frame.write_into(&mut frame_buf)?;

        self.send_raw_data(&frame_buf[..len])?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacAddr {
    raw: [u8; 6],
}

impl MacAddr {
    pub const fn broadcast() -> MacAddr {
        MacAddr {
            raw: [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        }
    }

    pub const fn zero() -> MacAddr {
        MacAddr {
            raw: [0x0, 0x0, 0x0, 0x0, 0x0, 0x0],
        }
    }
}

impl Display for MacAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        for (i, byte) in self.raw.iter().enumerate() {
            if i != 0 {
                write!(f, ":")?;
            }
            write!(f, "{:02X}", byte)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
pub enum EtherType {
    IPv4 = 0x0800,
    ARP = 0x0806,
}

pub const ETHERNET_HEADER_SIZE: usize = 6 + 6 + 2;

// See https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf#page85
// 802.1q VLAN Packet Format
#[repr(C, packed)]
pub struct EthernetFrame<'a> {
    pub dst_mac: MacAddr,     // 6 bytes
    pub src_mac: MacAddr,     // 6 bytes
    pub ethertype: EtherType, // 2 bytes
    pub payload: &'a [u8],    // ARP, IPv4, etc.
}

impl<'a> EthernetFrame<'a> {
    pub fn new(
        dst_mac: MacAddr,
        src_mac: MacAddr,
        ethertype: EtherType,
        payload: &'a [u8],
    ) -> Self {
        Self {
            dst_mac,
            src_mac,
            ethertype,
            payload,
        }
    }

    pub const fn header_len() -> usize {
        ETHERNET_HEADER_SIZE
    }

    pub const fn wire_len(&self) -> usize {
        ETHERNET_HEADER_SIZE + self.payload.len()
    }

    pub fn write_into(&self, buf: &mut [u8]) -> Result<usize, &'static str> {
        let len = self.wire_len();
        if buf.len() < len {
            return Err("buffer too small for frame");
        }

        buf[0..6].copy_from_slice(&self.dst_mac.raw);
        buf[6..12].copy_from_slice(&self.src_mac.raw);
        buf[12..14].copy_from_slice(&(self.ethertype as u16).to_be_bytes());
        buf[14..len].copy_from_slice(self.payload);

        Ok(len)
    }
}
