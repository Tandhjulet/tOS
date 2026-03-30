use core::net::Ipv4Addr;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::networking::NETWORK_INFO;
use crate::networking::protocols::dhcp::EnsureDHCPLease;
use crate::networking::protocols::ip::{IP, IPProtocol};
use crate::networking::protocols::socket::{RecvPacket, SOCKET_TABLE};
use crate::{helpers, println};

const TCPFLAG_CWR: u8 = 0b1000_0000;
const TCPFLAG_ECE: u8 = 0b0100_0000;
const TCPFLAG_URG: u8 = 0b0010_0000;
const TCPFLAG_ACK: u8 = 0b0001_0000;
const TCPFLAG_PSH: u8 = 0b0000_1000;
const TCPFLAG_RST: u8 = 0b0000_0100;
const TCPFLAG_SYN: u8 = 0b0000_0010;
const TCPFLAG_FIN: u8 = 0b0000_0001;

pub struct TcpConnection {
    dst_ip: Ipv4Addr,

    src_port: u16,
    dst_port: u16,

    ack_num: u32,
    seq_num: u32,

    state: TcpState,
}

impl TcpConnection {
    pub fn new(dst_ip: Ipv4Addr, src_port: u16, dst_port: u16) -> Self {
        Self {
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

        let syn = TcpMessage::new(&self, TCPFLAG_SYN, &[]);
        self.send_packet(&syn).await?;

        println!("sent packet!");

        let data = self.recv_packet().await;
        println!("received syn_ack!");
        Ok(())
    }

    async fn send_packet(&self, message: &TcpMessage<'_>) -> Result<(), String> {
        EnsureDHCPLease.await;

        let src = NETWORK_INFO.read().ip().unwrap();
        let data = message.to_payload();

        IP::send_packet(src, self.dst_ip, IPProtocol::TCP, &data).await?;
        Ok(())
    }
}

pub struct TcpMessage<'a> {
    connection: &'a TcpConnection,

    flags: u8,
    checksum: u16,
    data: &'a [u8],
}

impl<'a> TcpMessage<'a> {
    pub fn new(conn: &'a TcpConnection, flags: u8, data: &'a [u8]) -> Self {
        Self {
            connection: conn,
            flags,
            checksum: 0,
            data,
        }
    }

    pub fn calculate_sum(&self, include_checksum: bool) -> u32 {
        let src = NETWORK_INFO.read().ip().unwrap();
        let mut sum: u32 = 0;

        sum += u16::from_be_bytes([src.octets()[0], src.octets()[1]]) as u32;
        sum += u16::from_be_bytes([src.octets()[2], src.octets()[3]]) as u32;

        let dst_addr = self.connection.dst_ip;
        sum += u16::from_be_bytes([dst_addr.octets()[0], dst_addr.octets()[1]]) as u32;
        sum += u16::from_be_bytes([dst_addr.octets()[2], dst_addr.octets()[3]]) as u32;

        sum += 0; // zeros
        sum += IPProtocol::TCP as u32;
        let tcp_len = (self.data_offset() as usize * 4) + self.data.len();
        sum += tcp_len as u32;

        sum += self.connection.src_port as u32;
        sum += self.connection.dst_port as u32;

        sum += (self.connection.seq_num >> 16) as u32;
        sum += (self.connection.seq_num & 0xFFFF) as u32;
        sum += (self.connection.ack_num >> 16) as u32;
        sum += (self.connection.ack_num & 0xFFFF) as u32;

        sum += ((self.data_offset() as u32) << 12) | self.flags as u32;
        sum += 0x0FFF; // window size

        if include_checksum {
            sum += self.checksum as u32;
        }
        sum += 0; // urgent ptr

        for chunk in self.data.chunks(2) {
            let word = if chunk.len() == 2 {
                u16::from_be_bytes([chunk[0], chunk[1]])
            } else {
                u16::from_be_bytes([chunk[0], 0])
            };
            sum += word as u32;
        }

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

    pub fn data_offset(&self) -> u8 {
        // Header length in 32-bit words - we have no opts yet so it will be 5*4 = 20 bytes
        5
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let data_offset = self.data_offset();
        let buf_size = (data_offset as usize * 4) + self.data.len();

        let mut buffer = vec![0u8; buf_size];
        buffer[0..2].copy_from_slice(&self.connection.src_port.to_be_bytes());
        buffer[2..4].copy_from_slice(&self.connection.dst_port.to_be_bytes());
        buffer[4..8].copy_from_slice(&self.connection.seq_num.to_be_bytes());
        buffer[8..12].copy_from_slice(&self.connection.ack_num.to_be_bytes());
        buffer[12] = data_offset << 4;
        buffer[13] = self.flags;

        // FIXME: dont hardcode
        let window_size: u16 = 0x0FFF;
        buffer[14..16].copy_from_slice(&window_size.to_be_bytes());
        buffer[16..18].copy_from_slice(&self.calculate_send_checksum().to_be_bytes());
        // TODO: implement urgent ptrs
        buffer[18..20].copy_from_slice(&[0u8, 0u8]);

        buffer.extend_from_slice(&self.data);
        buffer
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
