use core::net::Ipv4Addr;

use num_enum::TryFromPrimitive;

use crate::networking::{
    MacAddr,
    protocols::{arp::HardwareType, udp::UdpMessage},
};

pub const DHCP_TRANSACTION_IDENTIFIER: u32 = 0x55555555;

pub struct DHCP {}

impl DHCP {
    pub fn discover() {}

    pub fn request(req_ip: Ipv4Addr) {}

    pub fn handle_packet(packet: UdpMessage) {}
}

// see https://datatracker.ietf.org/doc/html/rfc2131
pub struct DhcpMessage<'a> {
    op: u8,
    htype: u8,
    hlen: u8,
    hops: u8,

    xid: u32,

    secs: u16,
    flags: u16,

    ciaddr: Ipv4Addr,
    yiaddr: Ipv4Addr,
    siaddr: Ipv4Addr,
    giaddr: Ipv4Addr,

    // this field is actually 16 bytes to be future proof
    // how many bytes are used are defined by hlen (in this case, 6).
    chaddr: MacAddr,

    sname: &'a [u8; 64],
    file: &'a [u8; 128],
    options: &'a [u8; 128],
}

impl<'a> DhcpMessage<'a> {
    pub fn new() -> Self {
        // Self {
        //     op: DhcpMessageType::DhcpRequest as u8,
        //     htype: HardwareType::Ethernet as u8,
        //     hlen: 6,
        //     hops: 0,
        //     xid: DHCP_TRANSACTION_IDENTIFIER,
        //     secs: 0x0,
        //     flags: 0x0,
        //     ciaddr: (),
        //     yiaddr: (),
        //     siaddr: (),
        //     giaddr: (),
        //     chaddr: (),
        //     sname: (),
        //     file: (),
        //     options: (),
        // }
    }
}

#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(u8)]
pub enum DhcpMessageType {
    DhcpRequest = 0x1,
    DhcpReply = 0x2,
}
