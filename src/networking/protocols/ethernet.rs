use crate::{
    networking::{
        MacAddr, NETWORK_INFO, PacketBuf,
        protocols::{arp::Arp, ip::IP},
        queue_packet,
    },
    println,
};
use alloc::{format, string::String};
use num_enum::TryFromPrimitive;

pub const MTU: usize = 1500 /* bytes */;

pub struct Ethernet;

impl Ethernet {
    pub fn send_packet(
        dst_mac: MacAddr,
        ethertype: EtherType,
        mut buf: PacketBuf,
    ) -> Result<(), &'static str> {
        if buf.size() > MTU {
            panic!("Payload is too large!");
        }

        let src_mac = NETWORK_INFO.read().mac().unwrap();
        buf.write_header(EthernetHeader::len(), |wbuf| {
            EthernetHeader::write(dst_mac, src_mac, ethertype, wbuf);
        });

        queue_packet(buf.buf);
        Ok(())
    }

    pub fn handle_packet(mut buf: PacketBuf) -> Result<(), String> {
        let packet = EthernetHeader(buf.read_header(EthernetHeader::len()));

        let res = match packet.ethertype()? {
            EtherType::IPv4 => IP::handle_packet(buf),
            EtherType::ARP => Arp::handle_packet(buf),
        };

        res
    }
}

// See https://www.intel.com/content/dam/doc/manual/pci-pci-x-family-gbe-controllers-software-dev-manual.pdf#page85
// 802.1q VLAN Packet Format
#[repr(C, packed)]
pub struct EthernetHeader<'a>(&'a [u8]);

impl<'a> EthernetHeader<'a> {
    pub const fn len() -> usize {
        6 + 6 + 2
    }

    pub fn write(dst_mac: MacAddr, src_mac: MacAddr, ethertype: EtherType, buf: &mut [u8]) {
        buf[0..6].copy_from_slice(&dst_mac.octets());
        buf[6..12].copy_from_slice(&src_mac.octets());
        buf[12..14].copy_from_slice(&(ethertype as u16).to_be_bytes());
    }

    pub fn dst_mac(&self) -> MacAddr {
        MacAddr::from_bytes(&self.0[0..6])
    }

    pub fn src_mac(&self) -> MacAddr {
        MacAddr::from_bytes(&self.0[6..12])
    }

    pub fn ethertype(&self) -> Result<EtherType, String> {
        let raw_type = u16::from_be_bytes([self.0[12], self.0[13]]);
        let ethertype = EtherType::try_from(raw_type)
            .map_err(|v| format!("unknown ether type {}", v.number))?;
        Ok(ethertype)
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
