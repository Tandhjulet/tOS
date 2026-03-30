pub mod drivers;
pub mod protocols;

use core::{fmt::Display, net::Ipv4Addr, task::Poll};

use alloc::vec;
use alloc::{
    boxed::Box, collections::vec_deque::VecDeque, format, string::String, sync::Arc, vec::Vec,
};
use futures_util::task::AtomicWaker;
use num_enum::TryFromPrimitive;
use spin::{Mutex, RwLock};
use x86_64::{instructions::interrupts::without_interrupts, structures::idt::InterruptStackFrame};

use crate::{
    interrupts::{MIN_INTERRUPT, PICS},
    networking::{
        drivers::E1000,
        protocols::{arp::Arp, dhcp::DhcpLease, ip::IP},
    },
    println,
};

pub static NETWORK_DRIVER: Mutex<Option<Box<dyn NetworkDriver>>> = Mutex::new(None);
pub static NETWORK_INFO: RwLock<NetworkInfo> = RwLock::new(NetworkInfo::new());

static TX_QUEUE: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());
static TX_WAKER: AtomicWaker = AtomicWaker::new();

static RX_QUEUE: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());
static RX_WAKER: AtomicWaker = AtomicWaker::new();

pub struct NetworkInfo {
    mac: Option<MacAddr>,
    dhcp: Option<DhcpLease>,
}

impl NetworkInfo {
    pub const fn new() -> Self {
        Self {
            mac: None,
            dhcp: None,
        }
    }

    pub fn mac(&self) -> &Option<MacAddr> {
        &self.mac
    }

    pub fn dhcp(&self) -> &Option<DhcpLease> {
        &self.dhcp
    }

    pub fn ip(&self) -> Option<Ipv4Addr> {
        let Some(dhcp) = &self.dhcp else {
            return None;
        };
        Some(*dhcp.ip())
    }
}

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

    Arp::init();

    let driver = lock.as_mut().unwrap();
    driver.start();

    NETWORK_INFO.write().mac = Some(*driver.get_mac_addr());
}

pub trait NetworkDriver: Send {
    fn start(&mut self);
    fn get_mac_addr(&self) -> &MacAddr;
    fn is_up(&mut self) -> bool;
    fn prepare_transmit(&mut self, data: &[u8]);
    fn transmit(&mut self);
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
            let remapped_line = irq_line + (MIN_INTERRUPT as u8);
            PICS.lock().notify_end_of_interrupt(remapped_line);
        }

        RX_WAKER.wake();
    }

    pub fn send_packet(data: Vec<u8>) {
        {
            NETWORK_DRIVER
                .lock()
                .as_mut()
                .unwrap()
                .prepare_transmit(&data);
        }

        without_interrupts(|| {
            NETWORK_DRIVER.lock().as_mut().unwrap().transmit();
        })
    }
}

// tx
struct TxPacketFuture;

impl Future for TxPacketFuture {
    type Output = ();

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        TX_WAKER.register(cx.waker());

        if !TX_QUEUE.lock().is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

fn queue_packet(data: Vec<u8>) {
    TX_QUEUE.lock().push_back(data);
    TX_WAKER.wake();
}

pub async fn network_tx_task() {
    loop {
        TxPacketFuture.await;
        loop {
            let data = {
                let Some(data) = TX_QUEUE.lock().pop_front() else {
                    break;
                };
                data
            };

            <dyn NetworkDriver>::send_packet(data);
        }
    }
}

pub(self) fn send_packet(
    dst_mac: MacAddr,
    ethertype: EtherType,
    payload: &[u8],
) -> Result<(), &'static str> {
    // FIXME: dynamically allocate multiple ethernet frames
    const MAX_PAYLOAD_SIZE: usize = 1500 /* bytes */;
    if payload.len() > MAX_PAYLOAD_SIZE {
        panic!("Payload is too large!");
    }

    const FRAME_LEN: usize = EthernetFrame::header_len();
    let mut frame_buf = vec![0; FRAME_LEN + payload.len()];

    let frame = EthernetFrame {
        dst_mac,
        src_mac: NETWORK_INFO.read().mac().unwrap(),
        ethertype,
        payload,
    };

    frame.write_into(&mut frame_buf)?;
    queue_packet(frame_buf);
    Ok(())
}

// rx
struct RxPacketFuture;

impl Future for RxPacketFuture {
    type Output = ();

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        RX_WAKER.register(cx.waker());

        if !RX_QUEUE.lock().is_empty() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

pub async fn network_rx_task() {
    loop {
        RxPacketFuture.await;
        loop {
            let raw = {
                let Some(raw) = RX_QUEUE.lock().pop_front() else {
                    break;
                };
                raw
            };

            let packet = EthernetFrame::parse(&raw);
            if packet.is_err() {
                println!("NETWORK ERR: failed to parse ethernet frame!");
                return;
            }

            let packet = packet.unwrap();
            let res = match packet.ethertype {
                EtherType::IPv4 => IP::handle_packet(packet),
                EtherType::ARP => Arp::handle_packet(packet),
            };

            if let Err(msg) = res {
                println!("NETWORK ERR: {}", msg);
            }
        }
    }
}

//
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

    pub fn from_bytes(raw: &[u8]) -> Self {
        if raw.len() != 6 {
            panic!("Cannot create a MacAddr from {} bytes!", raw.len());
        }
        let mut bytes = [0u8; 6];
        bytes.copy_from_slice(raw);

        Self { raw: bytes }
    }

    pub fn octets(&self) -> [u8; 6] {
        self.raw
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

#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(u16)]
pub enum EtherType {
    IPv4 = 0x0800,
    ARP = 0x0806,
}

#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(u16)]
pub enum HardwareType {
    Ethernet = 0x1,
}

impl HardwareType {
    pub fn to_bytes(&self) -> [u8; 2] {
        (*self as u16).to_be_bytes()
    }
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

    pub fn parse(raw: &'a [u8]) -> Result<Self, String> {
        let dst_mac = MacAddr::from_bytes(&raw[0..6]);
        let src_mac = MacAddr::from_bytes(&raw[6..12]);

        let raw_type = u16::from_be_bytes([raw[12], raw[13]]);
        let ethertype = EtherType::try_from(raw_type)
            .map_err(|v| format!("unknown ether type {}", v.number))?;

        let payload = &raw[ETHERNET_HEADER_SIZE..];

        Ok(Self {
            dst_mac,
            src_mac,
            ethertype,
            payload,
        })
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
