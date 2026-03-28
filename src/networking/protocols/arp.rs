use core::{net::Ipv4Addr, task::Poll};

use alloc::{collections::btree_map::BTreeMap, string::String, sync::Arc};
use futures_util::task::AtomicWaker;
use num_enum::TryFromPrimitive;
use spin::Mutex;

use crate::networking::{self, EtherType, EthernetFrame, HardwareType, MacAddr, NETWORK_DRIVER};

static ARP_CACHE: Mutex<BTreeMap<Ipv4Addr, MacAddr>> = Mutex::new(BTreeMap::new());
static PENDING_ARP: Mutex<BTreeMap<Ipv4Addr, Arc<AtomicWaker>>> = Mutex::new(BTreeMap::new());

pub struct Arp {}

impl Arp {
    pub(in crate::networking) fn init() {
        // To allow DHCP, hardcode the broadcast entry
        ARP_CACHE
            .lock()
            .insert(Ipv4Addr::new(255, 255, 255, 255), MacAddr::broadcast());
    }

    fn send_request(ip: &Ipv4Addr) -> Result<(), &'static str> {
        let mac = {
            let mut lock = NETWORK_DRIVER.lock();
            let driver = lock.as_mut().unwrap();
            *driver.get_mac_addr()
        };

        const ARP_LEN: usize = ArpMessage::len();
        let mut arp_buf = [0u8; ARP_LEN];
        let arp = ArpMessage::new(mac, *ip);
        let arp_len = arp.write_to(&mut arp_buf);

        networking::send_packet(MacAddr::broadcast(), EtherType::ARP, &arp_buf[..arp_len])
    }

    pub async fn discover(ip: &Ipv4Addr) -> Result<MacAddr, &'static str> {
        let waker = Arc::new(AtomicWaker::new());
        PENDING_ARP.lock().insert(*ip, waker.clone());

        if let Err(msg) = Arp::send_request(ip) {
            PENDING_ARP.lock().remove(ip);
            return Err(msg);
        }

        Ok(ArpFuture { ip: *ip, waker }.await)
    }

    pub async fn lookup(ip: &Ipv4Addr) -> Option<MacAddr> {
        {
            let lock = ARP_CACHE.lock();
            if let Some(mac) = lock.get(ip) {
                return Some(*mac);
            }
        }

        Arp::discover(ip).await.ok()
    }

    pub fn handle_packet(packet: EthernetFrame) -> Result<(), String> {
        let arp_response = ArpMessage::from(&packet.payload[..ArpMessage::len()])?;

        ARP_CACHE
            .lock()
            .insert(arp_response.src_pc_addr, arp_response.src_hw_addr);

        let mut pending = PENDING_ARP.lock();
        if let Some(waker) = pending.remove(&arp_response.src_pc_addr) {
            waker.wake();
        }

        Ok(())
    }
}

struct ArpFuture {
    ip: Ipv4Addr,
    waker: Arc<AtomicWaker>,
}

impl Future for ArpFuture {
    type Output = MacAddr;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.waker.register(cx.waker());

        if let Some(mac) = ARP_CACHE.lock().get(&self.ip) {
            return Poll::Ready(*mac);
        }

        Poll::Pending
    }
}

impl Drop for ArpFuture {
    fn drop(&mut self) {
        PENDING_ARP.lock().remove(&self.ip);
    }
}

#[derive(Debug)]
pub struct ArpMessage {
    pub operation: Operation,

    pub src_hw_addr: MacAddr,
    pub src_pc_addr: Ipv4Addr,

    pub dst_hw_addr: MacAddr,
    pub dst_pc_addr: Ipv4Addr,
}

impl ArpMessage {
    pub fn new(src_mac: MacAddr, to_discover: Ipv4Addr) -> Self {
        const DST_HW_ADDR: MacAddr = MacAddr::zero();

        // hardcode IP until we get one
        const SRC_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 100, 2);

        Self {
            operation: Operation::ArpRequest,
            src_hw_addr: src_mac,
            src_pc_addr: SRC_IP,
            dst_hw_addr: DST_HW_ADDR,
            dst_pc_addr: to_discover,
        }
    }

    pub fn from(packet: &[u8]) -> Result<Self, &'static str> {
        if packet.len() != ArpMessage::len() {
            return Err("buffer is not correctly sized");
        }

        let raw_op = u16::from_be_bytes([packet[6], packet[7]]);
        let operation = Operation::try_from(raw_op).unwrap();

        let src_hw = MacAddr::from_bytes(&packet[8..14]);

        let mut raw_ip = [0u8; 4];
        raw_ip.copy_from_slice(&packet[14..18]);
        let src_ip = Ipv4Addr::new(raw_ip[0], raw_ip[1], raw_ip[2], raw_ip[3]);

        let dst_hw = MacAddr::from_bytes(&packet[18..24]);

        raw_ip.copy_from_slice(&packet[24..28]);
        let dst_ip = Ipv4Addr::new(raw_ip[0], raw_ip[1], raw_ip[2], raw_ip[3]);

        Ok(Self {
            operation,
            dst_hw_addr: dst_hw,
            dst_pc_addr: dst_ip,
            src_hw_addr: src_hw,
            src_pc_addr: src_ip,
        })
    }

    pub const fn len() -> usize {
        28
    }

    pub fn write_to(&self, buf: &mut [u8]) -> usize {
        buf[0..2].copy_from_slice(&HardwareType::Ethernet.to_bytes()); // HW Type
        buf[2..4].copy_from_slice(&ARPProtocolType::IPv4.to_bytes()); // Protocol type
        buf[4] = 6; // HW length
        buf[5] = 4; // Protocol length
        buf[6..8].copy_from_slice(&self.operation.to_bytes());
        buf[8..14].copy_from_slice(&self.src_hw_addr.raw);
        buf[14..18].copy_from_slice(&self.src_pc_addr.octets());
        buf[18..24].copy_from_slice(&self.dst_hw_addr.raw);
        buf[24..28].copy_from_slice(&self.dst_pc_addr.octets());

        ArpMessage::len()
    }
}

#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(u16)]
pub enum Operation {
    ArpRequest = 0x1,
    ArpResponse = 0x2,
    RarpRequest = 0x3,
    RarpResponse = 0x4,
}

impl Operation {
    pub fn to_bytes(&self) -> [u8; 2] {
        (*self as u16).to_be_bytes()
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u16)]
pub enum ARPProtocolType {
    IPv4 = 0x0800,
}

impl ARPProtocolType {
    pub fn to_bytes(&self) -> [u8; 2] {
        (*self as u16).to_be_bytes()
    }
}
