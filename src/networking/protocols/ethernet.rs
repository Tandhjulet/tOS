use crate::networking::{
    MacAddr, NETWORK_INFO,
    protocols::{arp::Arp, ip::IP},
    queue_packet,
};
use alloc::{format, string::String, vec};
use num_enum::TryFromPrimitive;

pub struct Ethernet;

impl Ethernet {
    pub fn send_packet(
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

    pub fn handle_packet(raw: &[u8]) -> Result<(), String> {
        let packet = EthernetFrame::parse(&raw)?;

        let res = match packet.ethertype {
            EtherType::IPv4 => IP::handle_packet(packet),
            EtherType::ARP => Arp::handle_packet(packet),
        };

        res
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
