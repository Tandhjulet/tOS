pub mod drivers;
pub mod protocols;

use core::{fmt::Display, net::Ipv4Addr, task::Poll};

use alloc::{boxed::Box, collections::vec_deque::VecDeque, sync::Arc, vec::Vec};
use futures_util::task::AtomicWaker;
use spin::{Mutex, RwLock};
use x86_64::{instructions::interrupts::without_interrupts, structures::idt::InterruptStackFrame};

use crate::networking::protocols::ethernet::Ethernet;
use crate::{
    interrupts::{MIN_INTERRUPT, PICS},
    networking::{
        drivers::E1000,
        protocols::{arp::Arp, dhcp::DhcpLease},
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

            let res = Ethernet::handle_packet(&raw);
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
