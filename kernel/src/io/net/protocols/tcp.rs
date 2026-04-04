use core::net::Ipv4Addr;

use alloc::borrow::ToOwned;
use alloc::format;
use alloc::string::String;

use crate::helpers;
use crate::io::net::protocols::dhcp::EnsureDHCPLease;
use crate::io::net::protocols::ip::{IP, IPProtocol, IpHeader};
use crate::io::net::protocols::socket::{RecvPacket, SOCKET_TABLE};
use crate::io::net::{NETWORK_INFO, PacketBuf};

pub mod flag {
    #[allow(unused)]
    pub const CWR: u8 = 0b1000_0000;
    pub const ECE: u8 = 0b0100_0000;
    pub const URG: u8 = 0b0010_0000;
    pub const ACK: u8 = 0b0001_0000;
    pub const PSH: u8 = 0b0000_1000;
    pub const RST: u8 = 0b0000_0100;
    pub const SYN: u8 = 0b0000_0010;
    pub const FIN: u8 = 0b0000_0001;
}

pub struct TcpConnection {
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,

    src_port: u16,
    dst_port: u16,

    ack_num: u32,
    seq_num: u32,

    state: TcpState,
}

impl TcpConnection {
    pub async fn new(dst_ip: Ipv4Addr, src_port: u16, dst_port: u16) -> Self {
        EnsureDHCPLease.await;

        let src_ip = NETWORK_INFO.read().ip().unwrap();
        Self {
            src_ip,
            dst_ip,

            src_port,
            dst_port,

            ack_num: 0,
            seq_num: 0,

            state: TcpState::CLOSED,
        }
    }

    pub async fn open(&mut self) -> Result<(), String> {
        SOCKET_TABLE.lock().bind(self.src_port, IPProtocol::TCP)?;

        if self.state != TcpState::CLOSED {
            return Err("Cannot open non-closed TCP conn!".to_owned());
        }

        // TODO: use random num
        self.seq_num = 0xdeadbeef;
        self.state = TcpState::SYNSENT;

        let synack = self.send_ack(flag::SYN, &[]).await?;
        if synack.flags() != flag::ACK | flag::SYN {
            return Err("Didn't receive SYN/ACK back from ACK!".to_owned());
        }

        self.state = TcpState::ESTABLISHED;
        self.send(flag::ACK, &[]).await?;

        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.state = TcpState::FINWAIT1;

        self.send(flag::FIN | flag::ACK, &[]).await?;
        let ack = self.recv_ack().await?;
        if ack.flags() != flag::ACK {
            return Err("Didn't receive ACK back from FIN!".to_owned());
        }

        self.state = TcpState::FINWAIT2;

        let finack = self.recv_ack().await?;
        if finack.flags() != flag::ACK | flag::FIN {
            return Err("Didn't receive FIN-ACK back from FIN!".to_owned());
        } else {
            self.seq_num += 1;
        };

        self.state = TcpState::TIMEWAIT;
        self.send(flag::ACK, &[]).await?;

        // TODO: normally wait for 240s before closing and releasing

        self.state = TcpState::CLOSED;
        SOCKET_TABLE.lock().unbind(self.src_port, IPProtocol::TCP);

        Ok(())
    }

    pub async fn recv_ack(&mut self) -> Result<TcpPacket, String> {
        let data = self.recv().await;
        let packet = TcpPacket::validated(self, data)?;
        self.ack_num = packet.seq_num() + packet.calc_seq_advance();
        Ok(packet)
    }

    pub async fn send_ack(&mut self, flags: u8, data: &[u8]) -> Result<TcpPacket, String> {
        let packet = TcpPacket::new(&self, flags, data);
        let seq_advance = packet.calc_seq_advance();
        self.send_packet(packet).await?;

        let recv = self.recv_ack().await?;
        if recv.flags() & flag::ACK > 0 {
            self.seq_num = self.seq_num + seq_advance;
        }

        Ok(recv)
    }

    pub async fn send(&mut self, flags: u8, data: &[u8]) -> Result<(), String> {
        let packet = TcpPacket::new(&self, flags, data);
        self.send_packet(packet).await?;
        Ok(())
    }

    pub async fn recv(&self) -> PacketBuf {
        RecvPacket {
            port: self.src_port,
            protocol: IPProtocol::TCP,
        }
        .await
    }

    async fn send_packet(&mut self, message: TcpPacket) -> Result<(), String> {
        IP::send_packet(self.src_ip, self.dst_ip, IPProtocol::TCP, message.buf).await?;
        Ok(())
    }
}

pub struct TcpPacket {
    src: Ipv4Addr,
    dst: Ipv4Addr,
    buf: PacketBuf,
}

impl TcpPacket {
    pub fn new(conn: &TcpConnection, flags: u8, data: &[u8]) -> Self {
        // FIXME
        let ip_opt_cnt = 0;
        let header_offset = 5;

        let mut buf = PacketBuf::new(
            TcpPacket::calculate_headroom(ip_opt_cnt, header_offset),
            data.len(),
            |buf| {
                buf.copy_from_slice(data);
            },
        );

        buf.write_header(header_offset * 4, |buf| {
            buf[0..2].copy_from_slice(&conn.src_port.to_be_bytes());
            buf[2..4].copy_from_slice(&conn.dst_port.to_be_bytes());
            buf[4..8].copy_from_slice(&conn.seq_num.to_be_bytes());
            buf[8..12].copy_from_slice(&conn.ack_num.to_be_bytes());
            buf[12] = (header_offset as u8) << 4;
            buf[13] = flags;

            let window_size = 0x0FFFu16;
            buf[14..16].copy_from_slice(&window_size.to_be_bytes());

            // TODO: implement urgent ptrs
            buf[18..20].copy_from_slice(&[0u8, 0u8]);
        });

        let mut packet = Self {
            src: conn.src_ip,
            dst: conn.dst_ip,
            buf,
        };

        let checksum = packet.calculate_send_checksum();
        packet.buf.patch_header(16, &checksum.to_be_bytes());

        packet
    }

    pub fn calculate_headroom(option_cnt: usize, header_offset: usize) -> usize {
        IpHeader::calculate_headroom(5 + option_cnt) + header_offset * 4
    }

    pub fn data_offset(&self) -> u8 {
        self.raw()[12] >> 4
    }

    pub fn data(&self) -> &[u8] {
        let data_start = (self.data_offset() as usize) * 4;
        &self.raw()[data_start..]
    }

    pub fn options(&self) -> &[u8] {
        let data_start = (self.data_offset() as usize) * 4;
        &self.raw()[20..data_start]
    }

    pub fn header(&self) -> &[u8] {
        &self.raw()[0..20]
    }

    pub fn raw(&self) -> &[u8] {
        self.buf.data()
    }

    pub fn ack_num(&self) -> u32 {
        u32::from_be_bytes([self.raw()[8], self.raw()[9], self.raw()[10], self.raw()[11]])
    }

    pub fn seq_num(&self) -> u32 {
        u32::from_be_bytes([self.raw()[4], self.raw()[5], self.raw()[6], self.raw()[7]])
    }

    pub fn flags(&self) -> u8 {
        self.raw()[13]
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes([self.raw()[0], self.raw()[1]])
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes([self.raw()[2], self.raw()[3]])
    }

    pub fn window(&self) -> u16 {
        u16::from_be_bytes([self.raw()[14], self.raw()[15]])
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.raw()[16], self.raw()[17]])
    }

    pub fn urg(&self) -> u16 {
        u16::from_be_bytes([self.raw()[18], self.raw()[19]])
    }

    pub fn parse(conn: &TcpConnection, data: PacketBuf) -> Self {
        let packet = Self {
            // swap here as this is C->S traffic
            src: conn.dst_ip,
            dst: conn.src_ip,

            buf: data,
        };

        packet
    }

    pub fn validated(conn: &TcpConnection, data: PacketBuf) -> Result<Self, String> {
        let packet = TcpPacket::parse(conn, data);
        packet.validate(conn)?;

        Ok(packet)
    }

    fn calc_seq_advance(&self) -> u32 {
        let mut n = self.data().len() as u32;
        if self.flags() & (flag::SYN | flag::FIN) != 0 {
            n += 1;
        }
        n
    }

    pub fn validate(&self, conn: &TcpConnection) -> Result<(), String> {
        let checksum = self.calculate_recv_checksum();
        if checksum != 0xFFFF {
            return Err(format!(
                "Received checksum does not match calculated (got {})!",
                checksum
            ));
        }

        let ack_num = self.ack_num();
        if ack_num != conn.seq_num + 1 {
            return Err(format!(
                "Received ACK num {} does not match own SEQ num + 1: {}",
                ack_num,
                conn.seq_num + 1
            ));
        }

        if self.flags() & flag::RST > 0 {
            return Err(format!("Connection Reset by peer!"));
        }

        Ok(())
    }

    fn pseudo_header_sum(&self) -> u32 {
        let mut sum = 0u32;
        for addr in [self.src, self.dst] {
            let o = addr.octets();
            sum += u16::from_be_bytes([o[0], o[1]]) as u32;
            sum += u16::from_be_bytes([o[2], o[3]]) as u32;
        }
        sum += IPProtocol::TCP as u32;
        sum += self.raw().len() as u32;

        sum
    }

    pub fn calculate_recv_checksum(&self) -> u16 {
        helpers::fold_sum(helpers::sum_byte_arr(&self.raw()) + self.pseudo_header_sum())
    }

    pub fn calculate_send_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(
            helpers::sum_byte_arr(&self.raw()[..16])
                + helpers::sum_byte_arr(&self.raw()[18..])
                + self.pseudo_header_sum(),
        );
        !sum
    }
}

#[derive(PartialEq, Eq, Debug)]
pub enum TcpState {
    SYNSENT,
    ESTABLISHED,
    FINWAIT1,
    FINWAIT2,
    CLOSEWAIT,
    CLOSING,
    LASTACK,
    TIMEWAIT,
    CLOSED,
}
