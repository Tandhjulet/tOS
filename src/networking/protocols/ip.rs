use core::net::Ipv4Addr;

use alloc::{format, string::String};
use num_enum::TryFromPrimitive;

use crate::helpers;
use crate::networking::protocols::arp::Arp;
use crate::networking::protocols::dhcp::EnsureDHCPLease;
use crate::networking::protocols::ethernet::{EtherType, Ethernet, EthernetHeader};
use crate::networking::protocols::socket::SOCKET_TABLE;
use crate::networking::{MacAddr, NETWORK_INFO, PacketBuf};

pub struct IP;

impl IP {
    // TODO: fragment big packets
    pub async fn send_packet(
        src: Ipv4Addr,
        dst: Ipv4Addr,
        protocol: IPProtocol,
        mut buf: PacketBuf,
    ) -> Result<(), String> {
        let ihl = 5;

        let data_len = buf.data().len();
        buf.write_header(ihl * 4, |wbuf| {
            IpHeader::write(src, dst, protocol, data_len, wbuf);
        });

        let mac = IP::get_route(dst).await?;

        Ethernet::send_packet(mac, EtherType::IPv4, buf)?;
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
    pub fn handle_packet(mut buf: PacketBuf) -> Result<(), String> {
        let ihl = (buf.peek(0) & 0x0F) as usize * 4;
        let header = IpHeader(buf.read_header(IpHeader::len(ihl)));

        // Some protocols, like IGMP, offload the checksum validation to hardware
        let proto = header.protocol()?;
        if proto.should_validate_checksum() {
            let checksum = header.calculate_recv_checksum();
            if checksum != 0xFFFF && checksum != 0 {
                return Err(format!(
                    "Checksum did not match for packet of type {:?}! Received {:#x} but expected 0xFFFF",
                    proto, checksum
                ));
            }
        }

        if header.version() != 4 {
            return Err(format!(
                "IP packet version {} is unsupported!",
                header.version()
            ));
        }

        match proto {
            IPProtocol::TCP | IPProtocol::UDP => {
                let dst_port = u16::from_be_bytes([buf.peek(2), buf.peek(3)]);

                SOCKET_TABLE.lock().deliver(dst_port, proto, buf);

                Ok(())
            }

            proto => Err(format!("unimplemented protocol: {:?}", proto)),
        }
    }
}

pub struct IpHeader<'a>(&'a [u8]);

impl<'a> IpHeader<'a> {
    pub fn write(
        src_addr: Ipv4Addr,
        dst_addr: Ipv4Addr,
        protocol: IPProtocol,
        data_len: usize,
        buf: &mut [u8],
    ) {
        // FIXME
        let ihl = 5;

        let header_len = (ihl * 4) as u16;
        let total_length = header_len + (data_len as u16);
        // let mut buf = vec![0u8; header_len as usize];

        let version = 4;
        buf[0] = (version << 4) | ihl;

        let dscp = 0;
        let ecn = 0;
        buf[1] = (dscp << 2) | ecn;

        buf[2..4].copy_from_slice(&total_length.to_be_bytes());

        let identification = 0u16;
        buf[4..6].copy_from_slice(&identification.to_be_bytes());

        let flags = 0u32;
        let fragment_offset = 0u16;
        buf[6..8].copy_from_slice(&(((flags as u16) << 13) | fragment_offset).to_be_bytes());

        let ttl = 64u8;
        buf[8] = ttl;
        buf[9] = protocol as u8;

        buf[12..16].copy_from_slice(&src_addr.octets());
        buf[16..20].copy_from_slice(&dst_addr.octets());
    }

    pub fn calculate_headroom(options: usize) -> usize {
        EthernetHeader::len() + options * 4
    }

    pub fn len(ihl: usize) -> usize {
        ihl * 4
    }

    pub fn ihl(&self) -> u8 {
        self.0[0] & 0xF
    }

    pub fn version(&self) -> u8 {
        self.0[0] >> 4
    }

    pub fn dscp(&self) -> u8 {
        self.0[1] >> 2
    }

    pub fn ecn(&self) -> u8 {
        self.0[1] & 0b11
    }

    pub fn length(&self) -> u16 {
        u16::from_be_bytes([self.0[2], self.0[3]])
    }

    pub fn identification(&self) -> u16 {
        u16::from_be_bytes([self.0[4], self.0[5]])
    }

    pub fn flags(&self) -> u8 {
        let flags_fragment = u16::from_be_bytes([self.0[6], self.0[7]]);
        (flags_fragment >> 13) as u8
    }

    pub fn fragment_offset(&self) -> u16 {
        let flags_fragment = u16::from_be_bytes([self.0[6], self.0[7]]);
        flags_fragment & 0x1FFF
    }

    pub fn ttl(&self) -> u8 {
        self.0[8]
    }

    pub fn protocol(&self) -> Result<IPProtocol, String> {
        let raw_protocol = self.0[9];
        let protocol = IPProtocol::try_from(raw_protocol)
            .map_err(|err| format!("failed to parse {:#x} as an IP protocol", err.number))?;
        Ok(protocol)
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.0[10], self.0[11]])
    }

    pub fn src(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.0[12], self.0[13], self.0[14], self.0[15])
    }

    pub fn dst(&self) -> Ipv4Addr {
        Ipv4Addr::new(self.0[16], self.0[17], self.0[18], self.0[19])
    }

    pub fn options(&self) -> &[u8] {
        &self.0[20..]
    }

    pub fn calculate_send_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(helpers::sum_byte_arr(&self.0));
        !sum
    }

    pub fn calculate_recv_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(
            helpers::sum_byte_arr(&self.0[..10]) + helpers::sum_byte_arr(&self.0[12..]),
        );
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
