use core::net::Ipv4Addr;

use alloc::collections::btree_map::BTreeMap;
use spin::Mutex;

use crate::networking::{EtherType, MacAddr, NETWORK_DRIVER};

static ARP_CACHE: Mutex<BTreeMap<Ipv4Addr, MacAddr>> = Mutex::new(BTreeMap::new());

pub struct Arp {}

impl Arp {
    pub fn discover(ip: &Ipv4Addr) -> Result<(), &'static str> {
        let mut lock = NETWORK_DRIVER.lock();
        let driver = lock.as_mut().unwrap();
        let mac = driver.get_mac_addr();

        const ARP_LEN: usize = ArpMessage::len();
        let mut arp_buf = [0u8; ARP_LEN];
        let arp = ArpMessage::new(*mac, *ip);
        let arp_len = arp.write_to(&mut arp_buf);

        // FIXME: make some sort of async waker such this can return when response is gotten
        driver.send_packet(MacAddr::broadcast(), EtherType::ARP, &arp_buf[..arp_len])?;
        Ok(())
    }

    pub fn lookup(ip: &Ipv4Addr) -> Option<MacAddr> {
        let lock = ARP_CACHE.lock();
        let cache_res = lock.get(ip);
        if let Some(mac) = cache_res {
            return Some(*mac);
        }
        drop(lock);

        Arp::discover(ip).unwrap();
        None
    }
}

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
        const SRC_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 100, 1);

        Self {
            operation: Operation::ArpRequest,
            src_hw_addr: src_mac,
            src_pc_addr: SRC_IP,
            dst_hw_addr: DST_HW_ADDR,
            dst_pc_addr: to_discover,
        }
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

#[derive(Debug, Clone, Copy)]
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
pub enum HardwareType {
    Ethernet = 0x1,
}

impl HardwareType {
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
