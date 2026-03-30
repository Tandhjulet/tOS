use core::net::Ipv4Addr;

use alloc::vec;
use alloc::{format, string::String, vec::Vec};

use crate::networking::protocols::dhcp::{DHCP, DHCP_CLIENT_PORT};
use crate::networking::protocols::ip::{IP, IPPacket, IPProtocol};

pub struct UDP;

impl UDP {
    pub async fn send_packet(
        src_ip: Ipv4Addr,
        dst_ip: Ipv4Addr,
        src_port: u16,
        dst_port: u16,
        data: &[u8],
    ) -> Result<(), String> {
        let message = UdpMessage::new(src_port, dst_port, data);
        let payload = message.to_payload();

        IP::send_packet(src_ip, dst_ip, IPProtocol::UDP, &payload).await?;
        Ok(())
    }

    pub fn handle_packet(packet: IPPacket) -> Result<(), String> {
        let message = UdpMessage::from(packet.data);

        if !message.validate_checksum() {
            return Err(format!(
                "Checksum {} does not match expected {}!",
                message.checksum,
                message.calculate_checksum()
            ));
        }

        // FIXME: abstract this dynamically
        if message.dst_port == DHCP_CLIENT_PORT {
            DHCP::handle_packet(message)?;
        }

        Ok(())
    }
}

pub struct UdpMessage<'a> {
    src_port: u16,
    dst_port: u16,

    length: u16,
    checksum: u16,

    data: &'a [u8],
}

impl<'a> UdpMessage<'a> {
    pub fn new(src_port: u16, dst_port: u16, data: &'a [u8]) -> Self {
        UdpMessage {
            src_port,
            dst_port,
            length: (data.len() + UdpMessage::header_len()) as u16,
            checksum: 0,
            data,
        }
    }

    pub fn from(payload: &'a [u8]) -> Self {
        let src_port = u16::from_be_bytes([payload[0], payload[1]]);
        let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
        let length = u16::from_be_bytes([payload[4], payload[5]]);
        let checksum = u16::from_be_bytes([payload[6], payload[7]]);

        let data = &payload[8..(length as usize)];

        Self {
            src_port,
            dst_port,
            length,
            checksum,
            data,
        }
    }

    pub const fn header_len() -> usize {
        8
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let mut buf: Vec<u8> = vec![0u8; self.length as usize];

        buf[0..2].copy_from_slice(&self.src_port.to_be_bytes());
        buf[2..4].copy_from_slice(&self.dst_port.to_be_bytes());
        buf[4..6].copy_from_slice(&self.length.to_be_bytes());
        buf[6..8].copy_from_slice(&self.checksum.to_be_bytes());
        buf[8..(self.length as usize)].copy_from_slice(&self.data);

        buf
    }

    pub fn data(&self) -> &[u8] {
        self.data
    }

    // TODO: implement checksumming (it's optional in ipv4)
    pub fn calculate_checksum(&self) -> u16 {
        0
    }

    pub fn validate_checksum(&self) -> bool {
        true
    }
}
