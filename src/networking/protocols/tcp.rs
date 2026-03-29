use core::net::Ipv4Addr;

use alloc::borrow::ToOwned;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::networking::protocols::ip::{IP, IPProtocol};
use crate::networking::{NETWORK_INFO, send_packet};
use crate::println;

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

    pub async fn open(&mut self) -> Result<(), String> {
        if self.state != TcpState::CLOSED {
            return Err("Cannot open non-closed TCP conn!".to_owned());
        }

        // TODO: use random num
        self.seq_num = 0xdeadbeef;
        self.state = TcpState::SYNSENT;

        let flags = TcpFlags::SYN;
        let message = TcpMessage::new(&self, flags, &[]);
        self.send_packet(&message).await?;
        Ok(())
    }

    async fn send_packet(&self, message: &TcpMessage<'_>) -> Result<(), String> {
        let src = NETWORK_INFO
            .read()
            .ip()
            .ok_or("Cannot establish TCP conn without DHCP lease")?;

        let data = message.to_payload();

        IP::send_packet(src, self.dst_ip, IPProtocol::TCP, &data).await?;
        Ok(())
    }
}

pub struct TcpMessage<'a> {
    connection: &'a TcpConnection,

    flags: TcpFlags,
    data: &'a [u8],
}

impl<'a> TcpMessage<'a> {
    pub fn new(conn: &'a TcpConnection, flags: TcpFlags, data: &'a [u8]) -> Self {
        Self {
            connection: conn,
            flags,
            data,
        }
    }

    pub fn calculate_checksum(&self) -> u16 {
        0
    }

    pub fn data_offset(&self) -> u8 {
        // Header length in 32-bit words - we have no opts yet so it will be 5*4 = 20 bytes
        5
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let data_offset = self.data_offset();
        let buf_size = (data_offset as usize) + self.data.len();

        let mut buffer = vec![0u8; buf_size];
        buffer[0..2].copy_from_slice(&self.connection.src_port.to_be_bytes());
        buffer[2..4].copy_from_slice(&self.connection.dst_port.to_be_bytes());
        buffer[4..8].copy_from_slice(&self.connection.seq_num.to_be_bytes());
        buffer[8..12].copy_from_slice(&self.connection.ack_num.to_be_bytes());
        buffer[12] = data_offset << 4;
        buffer[13] = self.flags.bits();

        // FIXME: dont hardcode
        let window_size: u16 = 0x0FFF;
        buffer[14..16].copy_from_slice(&window_size.to_be_bytes());
        buffer[16..18].copy_from_slice(&self.calculate_checksum().to_be_bytes());
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

bitflags::bitflags! {
    pub struct TcpFlags: u8 {
        const CWR = 0b1000_0000;
        const ECE = 0b0100_0000;
        const URG = 0b0010_0000;
        const ACK = 0b0001_0000;
        const PSH = 0b0000_1000;
        const RST = 0b0000_0100;
        const SYN = 0b0000_0010;
        const FIN = 0b0000_0001;
    }
}
