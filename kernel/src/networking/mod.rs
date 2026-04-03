pub mod drivers;
pub mod protocols;

use core::{fmt::Display, net::Ipv4Addr, task::Poll};

use alloc::vec;
use alloc::{boxed::Box, collections::vec_deque::VecDeque, sync::Arc, vec::Vec};
use futures_util::task::AtomicWaker;
use spin::{Mutex, RwLock};

use crate::networking::drivers::NetworkDriver;
use crate::networking::drivers::e1000::E1000;
use crate::networking::protocols::ethernet::Ethernet;
use crate::{
    networking::protocols::{arp::Arp, dhcp::DhcpLease},
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
    {
        let device = {
            let mut devices = crate::pci::DEVICES.lock();

            // https://wiki.osdev.org/PCI#Class_Codes
            let device = devices.iter_mut().find(|d| {
                let d = d.lock();
                d.class() == 0x2 && d.subclass() == 0x0
            });

            let Some(device) = device else { return };

            Arc::clone(device)
        };

        // FIXME: make this dynamic
        let Ok(driver) = E1000::new(device) else {
            println!("Could not configure network driver!");
            return;
        };
        *NETWORK_DRIVER.lock() = Some(Box::new(driver));
    };

    Arp::init();

    let mut lock = NETWORK_DRIVER.lock();
    let driver = lock.as_mut().unwrap();
    driver.start();

    NETWORK_INFO.write().mac = Some(*driver.get_mac_addr());
}

pub struct PacketBuf {
    buf: Vec<u8>,
    head: usize,
    tail: usize,
}

impl PacketBuf {
    pub fn new(headroom: usize, data_len: usize, writer: impl FnOnce(&mut [u8])) -> Self {
        let mut buf = vec![0u8; headroom + data_len];
        writer(&mut buf[headroom..]);

        let len = buf.len();

        Self {
            buf,
            head: headroom,
            tail: len,
        }
    }

    pub fn size(&self) -> usize {
        self.buf.len()
    }

    pub fn write_header(&mut self, len: usize, writer: impl FnOnce(&mut [u8])) {
        self.head -= len;
        writer(&mut self.buf[self.head..self.head + len])
    }

    pub fn patch_header(&mut self, offset: usize, data: &[u8]) {
        let start = self.head + offset;
        let idx = start + data.len();
        self.assert_can_idx(idx);
        self.buf[start..idx].copy_from_slice(data);
    }

    pub fn from(buf: Vec<u8>) -> Self {
        let len = buf.len();
        Self {
            buf,
            head: 0,
            tail: len,
        }
    }

    pub fn read_header(&mut self, len: usize) -> &[u8] {
        let idx = self.head + len;
        self.assert_can_idx(idx);

        let header = &self.buf[self.head..idx];
        self.head += len;
        header
    }

    pub fn peek(&self, offset: usize) -> u8 {
        let idx = self.head + offset;

        self.assert_can_idx(idx);
        self.buf[idx]
    }

    pub fn data(&self) -> &[u8] {
        &self.buf[self.head..self.tail]
    }

    fn assert_can_idx(&self, idx: usize) {
        assert!(
            idx <= self.tail,
            "cannot idx to buf at {}: 0 (START) -> {} (HEADER END) -> {} (DATA END) -> {} (BUFFER LEN)",
            idx,
            self.head,
            self.tail,
            self.buf.len()
        );
    }

    pub fn trim_end(&mut self, amt: usize) {
        self.tail -= amt;
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

pub(self) fn queue_packet(data: Vec<u8>) {
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

            let res = Ethernet::handle_packet(PacketBuf::from(raw));
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
