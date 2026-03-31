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

        let syn = TcpPacket::new(&self, TCPFLAG_SYN, &[]);
        self.send_packet(&syn).await?;

        let data = self.recv_packet().await;
        let synack = TcpPacket::parse_and_ack(self, &data)?;
        if synack.flags != TCPFLAG_ACK | TCPFLAG_SYN {
            return Err("Didn't receive SYN/ACK back from ACK!".to_owned());
        }

        self.state = TcpState::ESTABLISHED;

        let ack = TcpPacket::new(&self, TCPFLAG_ACK, &[]);
        self.send_packet(&ack).await?;

        Ok(())
    }

    pub async fn close(&mut self) -> Result<(), String> {
        self.state = TcpState::FINWAIT1;
        let fin = TcpPacket::new(&self, TCPFLAG_FIN, &[]);
        self.send_packet(&fin).await?;

        let data = self.recv_packet().await;
        let ack = TcpPacket::parse_and_ack(self, &data)?;

        self.state = TcpState::FINWAIT2;

        Ok(())
    }

    async fn send_packet(&mut self, message: &TcpPacket<'_>) -> Result<(), String> {
        let data = message.to_payload();

        self.seq_num = self.seq_num + message.calc_seq_advance();
        IP::send_packet(self.src_ip, self.dst_ip, IPProtocol::TCP, &data).await?;
        Ok(())
    }
}

pub struct TcpPacket<'a> {
    src: Ipv4Addr,
    dst: Ipv4Addr,

    src_port: u16,
    dst_port: u16,

    flags: u8,
    checksum: u16,
    data: &'a [u8],
    options: &'a [u8],

    header_offset: u8,
    window_size: u16,

    seq_num: u32,
    ack_num: u32,
}

impl<'a> TcpPacket<'a> {
    pub fn new(connection: &TcpConnection, flags: u8, data: &'a [u8]) -> Self {
        Self {
            src: connection.src_ip,
            dst: connection.dst_ip,
            dst_port: connection.dst_port,
            src_port: connection.src_port,

            checksum: 0,
            flags,
            data,
            options: &[],

            // FIXME
            header_offset: 5,
            window_size: 0x0FFF,

            seq_num: connection.seq_num,
            ack_num: connection.ack_num,
        }
    }

    pub fn parse_and_ack(conn: &mut TcpConnection, data: &'a [u8]) -> Result<Self, String> {
        let flags = data[13];

        let src_port = u16::from_be_bytes([data[0], data[1]]);
        let dst_port = u16::from_be_bytes([data[2], data[3]]);

        let seq = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
        let ack = u32::from_be_bytes([data[8], data[9], data[10], data[11]]);
        if flags & TCPFLAG_ACK > 0 && ack != conn.seq_num {
            return Err(format!(
                "received ack {} doesnt match seq+1: {}",
                ack, conn.seq_num
            ));
        }

        let offset = data[12] >> 4;

        let window = u16::from_be_bytes([data[14], data[15]]);
        let checksum = u16::from_be_bytes([data[16], data[17]]);
        let urg = u16::from_be_bytes([data[18], data[19]]);

        let data_start = (offset as usize) * 4;
        let options = &data[20..data_start];
        let payload = &data[data_start..];

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
            data: payload,
            options,
            ack_num: ack,
            seq_num: seq,
        };

        packet.validate(conn)?;
        conn.ack_num = packet.seq_num + packet.calc_seq_advance();

        Ok(packet)
    }

    fn calc_seq_advance(&self) -> u32 {
        let mut n = self.data.len() as u32;
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
                self.ack_num,
                conn.seq_num + 1
            ));
        }

        Ok(())
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let buf_size = (self.header_offset as usize * 4) + self.data.len();

        let mut buffer = vec![0u8; buf_size];
        buffer[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        buffer[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        buffer[4..8].copy_from_slice(&self.seq_num.to_be_bytes());
        buffer[8..12].copy_from_slice(&self.ack_num.to_be_bytes());
        buffer[12] = self.header_offset << 4;
        buffer[13] = self.flags;

        // FIXME: dont hardcode
        buffer[14..16].copy_from_slice(&self.window_size.to_be_bytes());
        buffer[16..18].copy_from_slice(&self.calculate_send_checksum().to_be_bytes());
        // TODO: implement urgent ptrs
        buffer[18..20].copy_from_slice(&[0u8, 0u8]);

        buffer.extend_from_slice(&self.data);
        buffer
    }

    pub fn calculate_sum(&self, include_checksum: bool) -> u32 {
        let mut sum: u32 = 0;

        for addr in [self.src, self.dst] {
            let octets = addr.octets();
            sum += u16::from_be_bytes([octets[0], octets[1]]) as u32;
            sum += u16::from_be_bytes([octets[2], octets[3]]) as u32;
        }
        sum += IPProtocol::TCP as u32;
        sum += ((self.header_offset as usize * 4) + self.data.len()) as u32;

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

        sum += helpers::sum_byte_arr(self.options);
        sum += helpers::sum_byte_arr(self.data);

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
