use core::net::Ipv4Addr;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::{format, vec};

use crate::helpers;
use crate::networking::NETWORK_INFO;
use crate::networking::protocols::dhcp::EnsureDHCPLease;
use crate::networking::protocols::ip::{IP, IPProtocol};
use crate::networking::protocols::socket::{RecvPacket, SOCKET_TABLE};

const TCPFLAG_CWR: u8 = 0b1000_0000;
const TCPFLAG_ECE: u8 = 0b0100_0000;
const TCPFLAG_URG: u8 = 0b0010_0000;
const TCPFLAG_ACK: u8 = 0b0001_0000;
const TCPFLAG_PSH: u8 = 0b0000_1000;
const TCPFLAG_RST: u8 = 0b0000_0100;
const TCPFLAG_SYN: u8 = 0b0000_0010;
const TCPFLAG_FIN: u8 = 0b0000_0001;

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

    async fn recv_packet(&self) -> Vec<u8> {
        RecvPacket {
            port: self.src_port,
            protocol: IPProtocol::TCP,
        }
        .await
    }

    pub async fn open(&mut self) -> Result<(), String> {
        SOCKET_TABLE.lock().bind(self.src_port, IPProtocol::TCP)?;

        if self.state != TcpState::CLOSED {
            return Err("Cannot open non-closed TCP conn!".to_owned());
        }

        // TODO: use random num
        self.seq_num = 0xdeadbeef;
        self.state = TcpState::SYNSENT;

        self.send(TCPFLAG_SYN, &[]).await?;

        let synack = self.recv_ack().await?;
        if synack.flags() != TCPFLAG_ACK | TCPFLAG_SYN {
            return Err("Didn't receive SYN/ACK back from ACK!".to_owned());
        }

        self.state = TcpState::ESTABLISHED;
        self.send(TCPFLAG_ACK, &[]).await?;

        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.state = TcpState::FINWAIT1;
        self.send(TCPFLAG_FIN | TCPFLAG_ACK, &[]).await?;

        let ack = self.recv_ack().await?;
        if ack.flags() != TCPFLAG_ACK {
            return Err("Didn't receive ACK back from FIN!".to_owned());
        }

        self.state = TcpState::FINWAIT2;

        let finack = self.recv_ack().await?;
        if finack.flags() != TCPFLAG_ACK | TCPFLAG_FIN {
            return Err("Didn't receive FIN-ACK back from FIN!".to_owned());
        }

        self.state = TcpState::TIMEWAIT;
        self.send(TCPFLAG_ACK, &[]).await?;

        // TODO: normally wait for 240s before closing and releasing

        self.state = TcpState::CLOSED;
        SOCKET_TABLE.lock().unbind(self.src_port, IPProtocol::TCP);

        Ok(())
    }

    pub async fn recv(&mut self) -> Result<TcpPacket, String> {
        self.recv_ack().await
    }

    pub async fn send(&mut self, flags: u8, data: &[u8]) -> Result<(), String> {
        let packet = TcpPacket::new(&self, flags, data);
        self.send_packet(&packet).await?;
        Ok(())
    }

    async fn send_packet(&mut self, message: &TcpPacket) -> Result<(), String> {
        let data = message.raw();

        self.seq_num = self.seq_num + message.calc_seq_advance();
        IP::send_packet(self.src_ip, self.dst_ip, IPProtocol::TCP, &data).await?;
        Ok(())
    }

    async fn recv_ack<'a>(&mut self) -> Result<TcpPacket, String> {
        let data = self.recv_packet().await;
        let packet = TcpPacket::parse(self, data)?;
        self.ack_num = packet.seq_num() + packet.calc_seq_advance();
        Ok(packet)
    }
}

pub struct TcpPacket {
    src: Ipv4Addr,
    dst: Ipv4Addr,
    buf: Vec<u8>,
}

impl TcpPacket {
    pub fn new(conn: &TcpConnection, flags: u8, data: &[u8]) -> Self {
        // FIXME
        let header_offset = 5;
        let buf_size = (header_offset as usize * 4) + data.len();

        let mut buf = vec![0u8; buf_size];
        buf[0..2].copy_from_slice(&conn.src_port.to_be_bytes());
        buf[2..4].copy_from_slice(&conn.dst_port.to_be_bytes());
        buf[4..8].copy_from_slice(&conn.seq_num.to_be_bytes());
        buf[8..12].copy_from_slice(&conn.ack_num.to_be_bytes());
        buf[12] = header_offset << 4;
        buf[13] = flags;

        // FIXME: dont hardcode
        let window_size = 0x0FFFu16;
        buf[14..16].copy_from_slice(&window_size.to_be_bytes());

        // TODO: implement urgent ptrs
        buf[18..20].copy_from_slice(&[0u8, 0u8]);

        buf.extend_from_slice(&data);

        let mut packet = Self {
            src: conn.src_ip,
            dst: conn.dst_ip,
            buf,
        };

        let checksum = packet.calculate_send_checksum();
        packet.buf[16..18].copy_from_slice(&checksum.to_be_bytes());

        packet
    }

    pub fn data_offset(&self) -> u8 {
        self.buf[12] >> 4
    }

    pub fn data(&self) -> &[u8] {
        let data_start = (self.data_offset() as usize) * 4;
        &self.buf[data_start..]
    }

    pub fn options(&self) -> &[u8] {
        let data_start = (self.data_offset() as usize) * 4;
        &self.buf[20..data_start]
    }

    pub fn header(&self) -> &[u8] {
        &self.buf[0..20]
    }

    pub fn raw(&self) -> &[u8] {
        &self.buf
    }

    pub fn ack_num(&self) -> u32 {
        u32::from_be_bytes([self.buf[8], self.buf[9], self.buf[10], self.buf[11]])
    }

    pub fn seq_num(&self) -> u32 {
        u32::from_be_bytes([self.buf[4], self.buf[5], self.buf[6], self.buf[7]])
    }

    pub fn flags(&self) -> u8 {
        self.buf[13]
    }

    pub fn src_port(&self) -> u16 {
        u16::from_be_bytes([self.buf[0], self.buf[1]])
    }

    pub fn dst_port(&self) -> u16 {
        u16::from_be_bytes([self.buf[2], self.buf[3]])
    }

    pub fn window(&self) -> u16 {
        u16::from_be_bytes([self.buf[14], self.buf[15]])
    }

    pub fn checksum(&self) -> u16 {
        u16::from_be_bytes([self.buf[16], self.buf[17]])
    }

    pub fn urg(&self) -> u16 {
        u16::from_be_bytes([self.buf[18], self.buf[19]])
    }

    pub fn parse(conn: &mut TcpConnection, data: Vec<u8>) -> Result<Self, String> {
        let packet = Self {
            // swap here as this is C->S traffic
            src: conn.dst_ip,
            dst: conn.src_ip,

            buf: data,
        };

        packet.validate(conn)?;
        Ok(packet)
    }

    fn calc_seq_advance(&self) -> u32 {
        let mut n = self.data().len() as u32;
        if self.flags() & (TCPFLAG_SYN | TCPFLAG_FIN) != 0 {
            n += 1;
        }
        n
    }

    pub fn validate(&self, conn: &TcpConnection) -> Result<(), String> {
        if self.calculate_recv_checksum() != 0xFFFF {
            return Err("Received checksum does not match calculated!".to_owned());
        }

        let ack_num = self.ack_num();
        if ack_num != conn.seq_num {
            return Err(format!(
                "Received ACK num {} does not match own SEQ num + 1: {}",
                self.ack_num(),
                conn.seq_num
            ));
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
        sum += self.buf.len() as u32;

        sum
    }

    pub fn calculate_recv_checksum(&self) -> u16 {
        helpers::fold_sum(helpers::sum_byte_arr(&self.buf) + self.pseudo_header_sum())
    }

    pub fn calculate_send_checksum(&mut self) -> u16 {
        let sum = helpers::fold_sum(
            000 + helpers::sum_byte_arr(&self.buf[..16])
                + helpers::sum_byte_arr(&self.buf[18..])
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
