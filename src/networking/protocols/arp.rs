use core::net::Ipv4Addr;

use crate::{
    networking::{EtherType, EthernetFrame, MacAddr, NETWORK_DRIVER},
    println,
};

pub struct Arp {}

impl Arp {
    pub fn send() -> Result<(), &'static str> {
        let mut lock = NETWORK_DRIVER.lock();
        let driver = lock.as_mut().unwrap();
        let mac = driver.get_mac_addr();

        const ARP_LEN: usize = ArpMessage::len();
        let mut arp_buf = [0u8; ARP_LEN];
        let arp = ArpMessage::new(*mac);
        let arp_len = arp.write_to(&mut arp_buf);

        let frame = EthernetFrame {
            dst_mac: MacAddr::broadcast(),
            src_mac: *mac,
            ethertype: EtherType::ARP,
            payload: &arp_buf[..arp_len],
        };

        const FRAME_LEN: usize = EthernetFrame::header_len();
        let mut frame_buf = [0u8; FRAME_LEN + ARP_LEN];
        let len = frame.write_into(&mut frame_buf)?;

        println!("sending...");
        driver.send_packet(&frame_buf[..len])?;
        println!("sent {:?}", &frame_buf[..len]);
        Ok(())
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
    pub fn new(src_mac: MacAddr) -> Self {
        const DST_HW_ADDR: MacAddr = MacAddr::zero();
        const DST_PC_ADDR: Ipv4Addr = Ipv4Addr::new(10, 2, 2, 1);

        // hardcode IP until we get one
        const SRC_IP: Ipv4Addr = Ipv4Addr::new(192, 168, 100, 1);

        Self {
            operation: Operation::ArpRequest,
            src_hw_addr: src_mac,
            src_pc_addr: SRC_IP,
            dst_hw_addr: DST_HW_ADDR,
            dst_pc_addr: DST_PC_ADDR,
        }
    }

    pub const fn len() -> usize {
        28
    }

    pub fn write_to(&self, buf: &mut [u8]) -> usize {
        buf[0..2].copy_from_slice(&HardwareType::Ethernet.to_bytes()); // HW Type
        buf[2..4].copy_from_slice(&ProtocolType::IPv4.to_bytes()); // Protocol type
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
pub enum ProtocolType {
    IPv4 = 0x0800,
}

impl ProtocolType {
    pub fn to_bytes(&self) -> [u8; 2] {
        (*self as u16).to_be_bytes()
    }
}
