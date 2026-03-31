use core::net::Ipv4Addr;

use alloc::vec;
use alloc::{string::String, vec::Vec};

use crate::helpers;
use crate::networking::protocols::ip::{IP, IPProtocol};
use crate::networking::protocols::socket::{RecvPacket, SOCKET_TABLE};

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

        IP::send_packet(self.src_ip, self.dst_ip, IPProtocol::UDP, packet.raw()).await?;
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
    buf: Vec<u8>,
}

impl UdpPacket {
    pub fn new(conn: &UdpConnection, data: &[u8]) -> Self {
        let buf_size = UdpPacket::header_len() + data.len();
        let mut buf = vec![0u8; buf_size];

        buf[0..2].copy_from_slice(&conn.src_port.to_be_bytes());
        buf[2..4].copy_from_slice(&conn.dst_port.to_be_bytes());
        buf[4..6].copy_from_slice(&(buf_size as u16).to_be_bytes());
        buf[6..8].copy_from_slice(&0u16.to_be_bytes());
        buf[8..].copy_from_slice(&data);

        let mut packet = UdpPacket {
            src: conn.src_ip,
            dst: conn.dst_ip,
            buf,
        };

        let checksum = packet.calculate_send_checksum();
        packet.buf[6..8].copy_from_slice(&checksum.to_be_bytes());

        packet
    }

    pub fn from(conn: &UdpConnection, payload: Vec<u8>) -> Self {
        Self {
            // Swapped as this is C->S
            src: conn.dst_ip,
            dst: conn.src_ip,
            buf: payload,
        }
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes([self.buf[0], self.buf[1]])
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes([self.buf[2], self.buf[3]])
    }

    pub fn len(&self) -> u16 {
        u16::from_be_bytes([self.buf[4], self.buf[5]])
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.buf[6], self.buf[7]])
    }

    pub fn data(&self) -> &[u8] {
        &self.buf[UdpPacket::header_len()..]
    }

    pub fn raw(&self) -> &[u8] {
        &self.buf
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
        sum += self.buf.len() as u32;

        sum
    }

    pub fn calculate_recv_checksum(&self) -> u16 {
        helpers::fold_sum(helpers::sum_byte_arr(&self.buf) + self.pseudo_header_sum())
    }

    pub fn calculate_send_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(
            000 + helpers::sum_byte_arr(&self.buf[..6])
                + helpers::sum_byte_arr(&self.buf[8..])
                + self.pseudo_header_sum(),
        );
        !sum
    }
}
