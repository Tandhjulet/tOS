use core::net::Ipv4Addr;

use alloc::vec;
use alloc::{format, string::String, vec::Vec};
use num_enum::TryFromPrimitive;

use crate::networking::protocols::dhcp::EnsureDHCPLease;
use crate::networking::protocols::ethernet::{EtherType, Ethernet, EthernetFrame};
use crate::networking::protocols::socket::SOCKET_TABLE;
use crate::networking::protocols::{arp::Arp, udp::UDP};
use crate::networking::{MacAddr, NETWORK_INFO};
use crate::{helpers, println};

pub struct IP;

impl IP {
    // TODO: fragment big packets
    pub async fn send_packet(
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: IPProtocol,
        data: &[u8],
    ) -> Result<(), String> {
        let packet = IPPacket::new(src, dst, protocol, data);

        let mac = IP::get_route(dst).await?;

        Ethernet::send_packet(mac, EtherType::IPv4, &packet.to_payload())?;
        Ok(())
    }

    pub async fn get_route(dst: Ipv4Addr) -> Result<MacAddr, String> {
        if dst.is_broadcast() {
            return Ok(MacAddr::broadcast());
        }

        EnsureDHCPLease.await;

        let (use_gateway, gateway) = {
            let lock = NETWORK_INFO.read();
            let lease = lock.dhcp().as_ref().unwrap();

            let ip = *lease.ip();
            let subnet = *lease.subnet_mask();

            let src_net = u32::from(ip) & u32::from(subnet);
            let dst_net = u32::from(dst) & u32::from(subnet);
            (src_net != dst_net, *lease.gateway())
        };

        let mac = Arp::lookup(if use_gateway { &gateway } else { &dst })
            .await
            .ok_or(format!("failed to find addr {} via ARP", dst))?;

        Ok(mac)
    }

    // TODO: handle fragmentation
    pub fn handle_packet(packet: EthernetFrame) -> Result<(), String> {
        let packet = IPPacket::from(packet.payload)?;
        let header = &packet.header;

        // Some protocols, like IGMP, offload the checksum validation to hardware
        if header.protocol.should_validate_checksum() {
            let checksum = header.calculate_recv_checksum();
            if checksum != 0xFFFF && checksum != 0 {
                return Err(format!(
                    "Checksum did not match for packet of type {:?}! Received {:#x} but expected 0xFFFF",
                    header.protocol, checksum
                ));
            }
        }

        if header.version != 4 {
            return Err(format!(
                "IP packet version {} is unsupported!",
                header.version
            ));
        }

        match header.protocol {
            IPProtocol::TCP => {
                let dst_port = u16::from_be_bytes([packet.data[2], packet.data[3]]);

                SOCKET_TABLE
                    .lock()
                    .deliver(dst_port, IPProtocol::TCP, Vec::from(packet.data));

                Ok(())
            }

            // FIXME: use socket table
            IPProtocol::UDP => UDP::handle_packet(packet),
            proto => Err(format!("unimplemented protocol: {:?}", proto)),
        }
    }
}

pub struct IPPacket<'a> {
    pub header: IPHeader,
    pub data: &'a [u8],
}

impl<'a> IPPacket<'a> {
    pub fn new(
        src_addr: Ipv4Addr,
        dst_addr: Ipv4Addr,
        protocol: IPProtocol,
        data: &'a [u8],
    ) -> Self {
        let mut header = IPHeader::new(src_addr, dst_addr, protocol, data.len() as u16);
        header.checksum = header.calculate_send_checksum();

        Self { header, data }
    }

    pub fn from(payload: &'a [u8]) -> Result<Self, String> {
        let header = IPHeader::from(payload)?;

        let data_range = header.header_len()..(header.total_length as usize);
        let data = &payload[data_range];

        Ok(Self { header, data })
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let header_len = self.header.header_len();

        let mut packet: Vec<u8> = vec![0u8; header_len + self.data.len()];
        self.header.write_into(&mut packet[..header_len]);
        packet[header_len..].copy_from_slice(self.data);

        packet
    }
}

pub struct IPHeader {
    version: u8,
    ihl: u8,
    dscp: u8,
    ecn: u8,
    total_length: u16,

    identification: u16,
    flags: u8,
    fragment_offset: u16,

    ttl: u8,
    protocol: IPProtocol,
    checksum: u16,

    src_addr: Ipv4Addr,
    dst_addr: Ipv4Addr,
}

impl IPHeader {
    pub fn new(
        src_addr: Ipv4Addr,
        dst_addr: Ipv4Addr,
        protocol: IPProtocol,
        data_len: u16,
    ) -> Self {
        let ihl = 5; // Specifices the number of 32-bit words in the header
        let header_len = (ihl * 4) as u16; // 4 bytes in each ihl

        let total_length = header_len + data_len;

        Self {
            version: 4, // IPv4 is always 4
            ihl,
            dscp: 0,      // Precedence, see https://en.wikipedia.org/wiki/Type_of_service
            ecn: 0,       // ECN is unsupported for now
            total_length, // Length of header + length of data
            identification: 0,
            // Fragmentation is unsupported for now
            flags: 0,
            fragment_offset: 0,

            ttl: 64, // Seconds before timeout
            protocol,
            checksum: 0,

            src_addr,
            dst_addr,
        }
    }

    pub fn header_len(&self) -> usize {
        (self.ihl as usize) * 4
    }

    pub fn from(packet: &[u8]) -> Result<Self, String> {
        let ihl = packet[0] & 0xF;
        let version = (packet[0] >> 4) & 0xF;

        let dscp = packet[1] >> 2;
        let ecn = packet[1] & 0b11;

        let length = u16::from_be_bytes([packet[2], packet[3]]);
        let identification = u16::from_be_bytes([packet[4], packet[5]]);

        let flags_fragment = u16::from_be_bytes([packet[6], packet[7]]);
        let flags = (flags_fragment >> 13) as u8;
        let fragment_offset = flags_fragment & 0x1FFF;

        let ttl = packet[8];
        let raw_protocol = packet[9];
        let protocol = IPProtocol::try_from(raw_protocol)
            .map_err(|err| format!("failed to parse {:#x} as an IP protocol", err.number))?;

        let checksum = u16::from_be_bytes([packet[10], packet[11]]);
        let src_addr = Ipv4Addr::new(packet[12], packet[13], packet[14], packet[15]);
        let dst_addr = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);

        Ok(Self {
            checksum,
            dscp,
            dst_addr,
            ecn,
            flags,
            fragment_offset,
            identification,
            ihl,
            total_length: length,
            protocol,
            src_addr,
            ttl,
            version,
        })
    }

    pub fn write_into(&self, buf: &mut [u8]) {
        buf[0] = (self.version << 4) | self.ihl;
        buf[1] = (self.dscp << 2) | self.ecn;
        buf[2..4].copy_from_slice(&self.total_length.to_be_bytes());
        buf[4..6].copy_from_slice(&self.identification.to_be_bytes());
        buf[6..8]
            .copy_from_slice(&(((self.flags as u16) << 13) | self.fragment_offset).to_be_bytes());
        buf[8] = self.ttl;
        buf[9] = self.protocol as u8;
        buf[10..12].copy_from_slice(&self.checksum.to_be_bytes());
        buf[12..16].copy_from_slice(&self.src_addr.octets());
        buf[16..20].copy_from_slice(&self.dst_addr.octets());
    }

    fn calculate_sum(&self, include_checksum: bool) -> u32 {
        let mut sum: u32 = 0;

        let first_word = ((self.version as u16) << 12)
            | ((self.ihl as u16) << 8)
            | ((self.dscp as u16) << 2)
            | (self.ecn as u16);
        sum += first_word as u32;

        sum += self.total_length as u32;
        sum += self.identification as u32;
        // Flags + Fragment Offset
        sum += ((self.flags as u16) << 13 | self.fragment_offset) as u32;
        // TTL + Protocol
        sum += ((self.ttl as u16) << 8 | self.protocol as u16) as u32;

        // Checksum field is zero during calculation
        if include_checksum {
            sum += self.checksum as u32;
        }

        // Source address
        sum += u16::from_be_bytes([self.src_addr.octets()[0], self.src_addr.octets()[1]]) as u32;
        sum += u16::from_be_bytes([self.src_addr.octets()[2], self.src_addr.octets()[3]]) as u32;
        // Destination address
        sum += u16::from_be_bytes([self.dst_addr.octets()[0], self.dst_addr.octets()[1]]) as u32;
        sum += u16::from_be_bytes([self.dst_addr.octets()[2], self.dst_addr.octets()[3]]) as u32;

        sum
    }

    pub fn calculate_send_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(self.calculate_sum(false));
        !sum
    }

    pub fn calculate_recv_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(self.calculate_sum(true));
        sum
    }
}

// https://en.wikipedia.org/wiki/IPv4#Protocol
#[derive(Debug, Clone, Copy, TryFromPrimitive, PartialEq, Eq)]
#[repr(u8)]
pub enum IPProtocol {
    ICMP = 1,
    IGMP = 2,
    TCP = 6,
    UDP = 17,
    ENCAP = 41,
    OSPF = 89,
    SCTP = 132,
}

impl IPProtocol {
    pub fn should_validate_checksum(&self) -> bool {
        *self == IPProtocol::TCP || *self == IPProtocol::UDP
    }
}
