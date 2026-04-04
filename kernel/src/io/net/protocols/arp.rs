use core::{net::Ipv4Addr, task::Poll};

use alloc::{collections::btree_map::BTreeMap, string::String, sync::Arc};
use futures_util::task::AtomicWaker;
use num_enum::TryFromPrimitive;
use spin::Mutex;

use crate::io::net::{
    MacAddr, NETWORK_INFO, PacketBuf,
    protocols::ethernet::{EtherType, Ethernet, EthernetHeader, HardwareType},
};

static ARP_CACHE: Mutex<BTreeMap<Ipv4Addr, MacAddr>> = Mutex::new(BTreeMap::new());
static PENDING_ARP: Mutex<BTreeMap<Ipv4Addr, Arc<AtomicWaker>>> = Mutex::new(BTreeMap::new());

pub struct Arp;

impl Arp {
    pub(in crate::io::net) fn init() {
        ARP_CACHE
            .lock()
            .insert(Ipv4Addr::new(255, 255, 255, 255), MacAddr::broadcast());
    }

    fn send_request(ip: &Ipv4Addr) -> Result<(), &'static str> {
        let mac = { NETWORK_INFO.read().mac().unwrap() };

        let packet = PacketBuf::new(EthernetHeader::len(), ArpPacket::len(), |buf| {
            ArpPacket::write(mac, *ip, buf);
        });

        Ethernet::send_packet(MacAddr::broadcast(), EtherType::ARP, packet)
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

    pub fn handle_packet(packet: PacketBuf) -> Result<(), String> {
        let arp_response = ArpPacket(packet.data());

        ARP_CACHE
            .lock()
            .insert(arp_response.src_ip(), arp_response.src_hw());

        let mut pending = PENDING_ARP.lock();
        if let Some(waker) = pending.remove(&arp_response.src_ip()) {
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

pub struct ArpPacket<'a>(&'a [u8]);

impl<'a> ArpPacket<'a> {
    pub fn write(src_mac: MacAddr, to_discover: Ipv4Addr, buf: &mut [u8]) {
        const DST_HW_ADDR: MacAddr = MacAddr::zero();

        let src_ip = match NETWORK_INFO.read().dhcp() {
            Some(lease) => *lease.ip(),
            None => Ipv4Addr::new(0, 0, 0, 0),
        };

        buf[0..2].copy_from_slice(&HardwareType::Ethernet.to_bytes()); // HW Type
        buf[2..4].copy_from_slice(&ARPProtocolType::IPv4.to_bytes()); // Protocol type
        buf[4] = 6; // HW length
        buf[5] = 4; // Protocol length
        buf[6..8].copy_from_slice(&Operation::ArpRequest.to_bytes());
        buf[8..14].copy_from_slice(&src_mac.octets());
        buf[14..18].copy_from_slice(&src_ip.octets());
        buf[18..24].copy_from_slice(&DST_HW_ADDR.octets());
        buf[24..28].copy_from_slice(&to_discover.octets());
    }

    pub fn operation(&self) -> Result<Operation, &'static str> {
        let raw_op = u16::from_be_bytes([self.0[6], self.0[7]]);
        let operation = Operation::try_from(raw_op).unwrap();
        Ok(operation)
    }

    pub fn src_hw(&self) -> MacAddr {
        MacAddr::from_bytes(&self.0[8..14])
    }

    pub fn src_ip(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.0[14], self.0[15], self.0[16], self.0[17])
    }

    pub fn dst_hw(&self) -> MacAddr {
        MacAddr::from_bytes(&self.0[18..24])
    }

    pub fn dst_ip(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.0[24], self.0[25], self.0[26], self.0[27])
    }

    pub const fn len() -> usize {
        28
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
