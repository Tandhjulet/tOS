use core::net::Ipv4Addr;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::sync::Arc;
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

        let syn = TcpPacket::new(&self, TCPFLAG_SYN, Vec::new());
        self.send_packet(&syn).await?;

        let synack = self.recv_ack().await?;
        if synack.flags != TCPFLAG_ACK | TCPFLAG_SYN {
            return Err("Didn't receive SYN/ACK back from ACK!".to_owned());
        }

        self.state = TcpState::ESTABLISHED;

        let ack = TcpPacket::new(&self, TCPFLAG_ACK, Vec::new());
        self.send_packet(&ack).await?;

        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.state = TcpState::FINWAIT1;
        let fin = TcpPacket::new(&self, TCPFLAG_ACK | TCPFLAG_FIN, Vec::new());
        self.send_packet(&fin).await?;

        let ack = self.recv_ack().await?;
        if ack.flags != TCPFLAG_ACK {
            return Err("Didn't receive ACK back from FIN!".to_owned());
        }

        self.state = TcpState::FINWAIT2;

        let finack = self.recv_ack().await?;
        if finack.flags != TCPFLAG_ACK | TCPFLAG_FIN {
            return Err("Didn't receive FIN-ACK back from FIN!".to_owned());
        }

        self.state = TcpState::TIMEWAIT;

        let ack = TcpPacket::new(&self, TCPFLAG_ACK, Vec::new());
        self.send_packet(&ack).await?;

        // TODO: normally wait for 240s before closing and releasing

        self.state = TcpState::CLOSED;

        SOCKET_TABLE.lock().unbind(self.src_port, IPProtocol::TCP);

        Ok(())
    }

    async fn send_packet(&mut self, message: &TcpPacket) -> Result<(), String> {
        let data = message.raw();

        self.seq_num = self.seq_num + message.calc_seq_advance();
        IP::send_packet(self.src_ip, self.dst_ip, IPProtocol::TCP, &data).await?;
        Ok(())
    }

    fn parse_and_ack<'a>(&mut self, data: Vec<u8>) -> Result<TcpPacket, String> {
        let packet = TcpPacket::parse(self, data)?;
        self.ack_num = packet.seq_num + packet.calc_seq_advance();
        Ok(packet)
    }

    async fn recv_ack<'a>(&mut self) -> Result<TcpPacket, String> {
        let data = self.recv_packet().await;
        let packet = self.parse_and_ack(data)?;
        Ok(packet)
    }
}

pub struct TcpPacket {
    src: Ipv4Addr,
    dst: Ipv4Addr,

    src_port: u16,
    dst_port: u16,

    flags: u8,
    checksum: u16,

    header_offset: u8,
    window_size: u16,

    seq_num: u32,
    ack_num: u32,

    buf: Vec<u8>,
}

impl TcpPacket {
    pub fn new(connection: &TcpConnection, flags: u8, data: Vec<u8>) -> Self {
        let mut packet = Self {
            src: connection.src_ip,
            dst: connection.dst_ip,
            dst_port: connection.dst_port,
            src_port: connection.src_port,

            checksum: 0,
            flags,

            buf: Vec::new(),

            // FIXME
            header_offset: 5,
            window_size: 0x0FFF,

            seq_num: connection.seq_num,
            ack_num: connection.ack_num,
        };

        let payload = packet.get_payload(&data, &[]);
        packet.buf = payload;

        packet
    }

    pub fn data(&self) -> &[u8] {
        let data_start = (self.header_offset as usize) * 4;
        &self.buf[data_start..]
    }

    pub fn options(&self) -> &[u8] {
        let data_start = (self.header_offset as usize) * 4;
        &self.buf[20..data_start]
    }

    pub fn header(&self) -> &[u8] {
        &self.buf[0..20]
    }

    pub fn raw(&self) -> &[u8] {
        &self.buf
    }

    pub fn parse(conn: &mut TcpConnection, data: Vec<u8>) -> Result<Self, String> {
        let flags = data[13];

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);

        let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);

        let offset = data[12] >> 4;

        let window = u16::from_be_bytes([data[14], data[15]]);
        let checksum = u16::from_be_bytes([data[16], data[17]]);
        let urg = u16::from_be_bytes([data[18], data[19]]);

        let packet = Self {
            // swap here as this is C->S traffic
            src: conn.dst_ip,
            dst: conn.src_ip,
            dst_port,
            src_port,

            flags,
            checksum,
            window_size: window,
            header_offset: offset,
            ack_num: ack,
            seq_num: seq,

            buf: data,
        };

        packet.validate(conn)?;
        Ok(packet)
    }

    fn calc_seq_advance(&self) -> u32 {
        let mut n = self.data().len() as u32;
        if self.flags & (TCPFLAG_SYN | TCPFLAG_FIN) != 0 {
            n += 1;
        }
        n
    }

    pub fn validate(&self, conn: &TcpConnection) -> Result<(), String> {
        if self.calculate_recv_checksum() != 0xFFFF {
            return Err("Received checksum does not match calculated!".to_owned());
        }

        if self.ack_num != conn.seq_num {
            return Err(format!(
                "Received ACK num {} does not match own SEQ num + 1: {}",
                self.ack_num, conn.seq_num
            ));
        }

        Ok(())
    }

    fn get_payload(&mut self, data: &[u8], options: &[u8]) -> Vec<u8> {
        let buf_size = (self.header_offset as usize * 4) + data.len();

        let mut buf = vec![0u8; buf_size];
        buf[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        buf[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        buf[4..8].copy_from_slice(&self.seq_num.to_be_bytes());
        buf[8..12].copy_from_slice(&self.ack_num.to_be_bytes());
        buf[12] = self.header_offset << 4;
        buf[13] = self.flags;

        // FIXME: dont hardcode
        buf[14..16].copy_from_slice(&self.window_size.to_be_bytes());

        // TODO: implement urgent ptrs
        buf[18..20].copy_from_slice(&[0u8, 0u8]);

        buf.extend_from_slice(data);

        // Don't use calculate_send_checksum here, as we the raw buffer isn't loaded yet
        let checksum = !helpers::fold_sum(self.calculate_sum(data, options, false));
        buf[16..18].copy_from_slice(&checksum.to_be_bytes());

        buf
    }

    fn calculate_sum(&self, data: &[u8], options: &[u8], include_checksum: bool) -> u32 {
        let mut sum: u32 = 0;

        for addr in [self.src, self.dst] {
            let octets = addr.octets();
            sum += u16::from_be_bytes([octets[0], octets[1]]) as u32;
            sum += u16::from_be_bytes([octets[2], octets[3]]) as u32;
        }
        sum += IPProtocol::TCP as u32;
        sum += ((self.header_offset as usize * 4) + data.len()) as u32;

        sum += self.src_port as u32;
        sum += self.dst_port as u32;

        sum += (self.seq_num >> 16) as u32;
        sum += (self.seq_num & 0xFFFF) as u32;
        sum += (self.ack_num >> 16) as u32;
        sum += (self.ack_num & 0xFFFF) as u32;

        sum += ((self.header_offset as u32) << 12) | self.flags as u32;
        sum += self.window_size as u32; // window size

        if include_checksum {
            sum += self.checksum as u32;
        }
        sum += 0; // urgent ptr

        sum += helpers::sum_byte_arr(options);
        sum += helpers::sum_byte_arr(data);

        sum
    }

    pub fn calculate_recv_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(self.calculate_sum(self.data(), self.options(), true));
        sum
    }

    pub fn calculate_send_checksum(&self) -> u16 {
        let sum = helpers::fold_sum(self.calculate_sum(self.data(), self.options(), false));
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
