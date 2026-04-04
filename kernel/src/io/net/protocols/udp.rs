use core::net::Ipv4Addr;

use alloc::{string::String, vec::Vec};

use crate::helpers;
use crate::io::net::PacketBuf;
use crate::io::net::protocols::ip::{IP, IPProtocol, IpHeader};
use crate::io::net::protocols::socket::{RecvPacket, SOCKET_TABLE};

pub struct UdpConnection {
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,

    src_port: u16,
    dst_port: u16,
}

impl UdpConnection {
    pub fn new(src_ip: Ipv4Addr, dst_ip: Ipv4Addr, src_port: u16, dst_port: u16) -> Self {
        Self {
            src_ip,
            dst_ip,
            dst_port,
            src_port,
        }
    }

    pub fn open(&mut self) -> Result<(), String> {
        SOCKET_TABLE.lock().bind(self.src_port, IPProtocol::UDP)?;
        Ok(())
    }

    pub async fn send(&self, data: &[u8]) -> Result<(), String> {
        let packet = UdpPacket::new(&self, data);

        IP::send_packet(self.src_ip, self.dst_ip, IPProtocol::UDP, packet.buf).await?;
        Ok(())
    }

    pub async fn recv(&self) -> Vec<u8> {
        let payload = RecvPacket {
            port: self.src_port,
            protocol: IPProtocol::UDP,
        }
        .await;

        let message = UdpPacket::from(self, payload);

        message.data().to_vec()
    }
}

pub struct UdpPacket {
    src: Ipv4Addr,
    dst: Ipv4Addr,
    buf: PacketBuf,
}

impl UdpPacket {
    pub fn new(conn: &UdpConnection, data: &[u8]) -> Self {
        let ip_opt_cnt = 0;
        let mut buf = PacketBuf::new(
            UdpPacket::calculate_headroom(ip_opt_cnt),
            data.len(),
            |buf| {
                buf.copy_from_slice(data);
            },
        );

        buf.write_header(UdpPacket::header_len(), |buf| {
            buf[0..2].copy_from_slice(&conn.src_port.to_be_bytes());
            buf[2..4].copy_from_slice(&conn.dst_port.to_be_bytes());

            let total_length = UdpPacket::header_len() + data.len();
            buf[4..6].copy_from_slice(&(total_length as u16).to_be_bytes());
        });

        let mut packet = UdpPacket {
            src: conn.src_ip,
            dst: conn.dst_ip,
            buf,
        };

        let checksum = packet.calculate_send_checksum();
        packet.buf.patch_header(6, &checksum.to_be_bytes());

        packet
    }

    pub fn calculate_headroom(option_cnt: usize) -> usize {
        UdpPacket::header_len() + IpHeader::calculate_headroom(5 + option_cnt)
    }

    pub fn from(conn: &UdpConnection, data: PacketBuf) -> Self {
        Self {
            // Swapped as this is C->S
            src: conn.dst_ip,
            dst: conn.src_ip,
            buf: data,
        }
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes([self.raw()[0], self.raw()[1]])
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes([self.raw()[2], self.raw()[3]])
    }

    pub fn len(&self) -> u16 {
        u16::from_be_bytes([self.raw()[4], self.raw()[5]])
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.raw()[6], self.raw()[7]])
    }

    pub fn data(&self) -> &[u8] {
        &self.raw()[UdpPacket::header_len()..]
    }

    pub fn raw(&self) -> &[u8] {
        &self.buf.data()
    }

    pub const fn header_len() -> usize {
        8
    }

    pub fn pseudo_header_sum(&self) -> u32 {
        let mut sum = 0u32;

        for addr in [self.src, self.dst] {
            let o = addr.octets();
            sum += u16::from_be_bytes([o[0], o[1]]) as u32;
            sum += u16::from_be_bytes([o[2], o[3]]) as u32;
        }
        sum += IPProtocol::UDP as u32;
        sum += self.raw().len() as u32;

        sum
    }

    pub fn calculate_recv_checksum(&self) -> u16 {
        helpers::fold_sum(helpers::sum_byte_arr(&self.raw()) + self.pseudo_header_sum())
    }

    pub fn calculate_send_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(
            000 + helpers::sum_byte_arr(&self.raw()[..6])
                + helpers::sum_byte_arr(&self.raw()[8..])
                + self.pseudo_header_sum(),
        );
        !sum
    }
}
