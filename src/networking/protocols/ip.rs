use core::net::Ipv4Addr;

use alloc::vec::Vec;

use crate::networking::{self, EtherType, EthernetFrame, MacAddr};

pub struct IP {
    _private: (),
}

impl IP {
    pub fn send_packet(src: Ipv4Addr, dst: Ipv4Addr, data: &[u8]) -> Result<(), &'static str> {
        let mut header = IPHeader::new(src, dst, data.len() as u16);
        header.checksum = header.calculate_checksum();

        let header_len = header.length();
        let mut packet: Vec<u8> = Vec::with_capacity(header_len + data.len());
        header.write_into(&mut packet[..header_len]);
        packet.extend_from_slice(data);

        // TODO: get mac with arp
        networking::send_packet(MacAddr::zero(), EtherType::IPv4, data)?;
        Ok(())
    }

    pub fn handle_packet(packet: EthernetFrame) -> Result<(), &'static str> {
        Ok(())
    }
}

pub struct IPHeader {
    version: u8,
    ihl: u8,
    dscp: u8,
    ecn: u8,
    length: u16,

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
    pub fn new(src_addr: Ipv4Addr, dst_addr: Ipv4Addr, data_len: u16) -> Self {
        let ihl = 5; // Specifices the number of 32-bit words in the header
        let header_len = (ihl * 4) as u16; // 4 bytes in each ihl

        let total_length = header_len + data_len;

        Self {
            version: 4, // IPv4 is always 4
            ihl,
            dscp: 0, // Precedence, see https://en.wikipedia.org/wiki/Type_of_service
            ecn: 0,  // ECN is unsupported for now
            length: total_length, // Length of header + length of data
            identification: 0,
            // Fragmentation is unsupported for now
            flags: 0,
            fragment_offset: 0,

            ttl: 64,                   // Seconds before timeout
            protocol: IPProtocol::UDP, // FIXME
            checksum: 0,

            src_addr,
            dst_addr,
        }
    }

    pub fn length(&self) -> usize {
        (self.ihl as usize) * 4
    }

    pub fn write_into(&self, buf: &mut [u8]) {
        buf[0] = (self.version << 4) | self.ihl;
        buf[1] = (self.dscp << 2) | self.ecn;
        buf[2..4].copy_from_slice(&self.length.to_be_bytes());
        buf[4..6].copy_from_slice(&self.identification.to_be_bytes());
        buf[6..8]
            .copy_from_slice(&(((self.flags as u16) << 13) | self.fragment_offset).to_be_bytes());
        buf[8] = self.ttl;
        buf[9] = self.protocol as u8;
        buf[10..12].copy_from_slice(&self.checksum.to_be_bytes());
        buf[12..16].copy_from_slice(&self.src_addr.octets());
        buf[16..20].copy_from_slice(&self.dst_addr.octets());
    }

    pub fn calculate_checksum(&mut self) -> u16 {
        let mut sum: u32 = 0;

        // Version + IHL
        sum += ((self.version as u16) << 8 | self.ihl as u16) as u32;
        // DSCP + ECN
        sum += ((self.dscp as u16) << 2 | self.ecn as u16) as u32;
        sum += self.length as u32;
        sum += self.identification as u32;
        // Flags + Fragment Offset
        sum += ((self.flags as u16) << 13 | self.fragment_offset) as u32;
        // TTL + Protocol
        sum += ((self.ttl as u16) << 8 | self.protocol as u16) as u32;
        // Checksum field is zero during calculation
        sum += 0u32;
        // Source address
        sum += u16::from_be_bytes([self.src_addr.octets()[0], self.src_addr.octets()[1]]) as u32;
        sum += u16::from_be_bytes([self.src_addr.octets()[2], self.src_addr.octets()[3]]) as u32;
        // Destination address
        sum += u16::from_be_bytes([self.dst_addr.octets()[0], self.dst_addr.octets()[1]]) as u32;
        sum += u16::from_be_bytes([self.dst_addr.octets()[2], self.dst_addr.octets()[3]]) as u32;

        // Fold 32-bit sum into 16 bits by adding the carry
        while sum >> 16 != 0 {
            sum = (sum & 0xFFFF) + (sum >> 16);
        }

        let final_sum = !(sum as u16);
        self.checksum = final_sum;
        final_sum
    }
}

// https://en.wikipedia.org/wiki/IPv4#Protocol
#[derive(Debug, Clone, Copy)]
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
