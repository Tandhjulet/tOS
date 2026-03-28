use core::net::Ipv4Addr;

use alloc::vec;
use alloc::{borrow::ToOwned, string::String, vec::Vec};
use num_enum::TryFromPrimitive;

use crate::networking::NETWORK_INFO;
use crate::networking::{
    EtherType, HardwareType, MacAddr, NETWORK_DRIVER,
    protocols::udp::{UDP, UdpMessage},
};
use crate::println;

pub const DHCP_SERVER_PORT: u16 = 67;
pub const DHCP_CLIENT_PORT: u16 = 68;
pub const DHCP_TRANSACTION_IDENTIFIER: u32 = 0x55555555;

pub struct DHCP {}

impl DHCP {
    pub async fn discover() {
        let mac = { NETWORK_INFO.read().mac().unwrap() };

        let options = DhcpOptionsBuilder::new()
            .message_type(DhcpMessageType::DhcpDiscover)
            .client_identifier(HardwareType::Ethernet, mac.octets().to_vec())
            .parameter_request_list([
                0x1,  // subnet mask
                0x3,  // router
                0x6,  // domain name server
                0xf,  // domain name
                0x39, // max. DHCP message size
            ])
            .build();

        let dhcp_broadcast = DhcpMessage::new(mac, "t_os".to_owned(), options);

        let src_ip = Ipv4Addr::new(0, 0, 0, 0);
        let dst_ip = Ipv4Addr::new(255, 255, 255, 255);

        UDP::send_packet(
            src_ip,
            dst_ip,
            DHCP_CLIENT_PORT,
            DHCP_SERVER_PORT,
            &dhcp_broadcast.to_payload(),
        )
        .await
        .unwrap();
    }

    pub fn request(req_ip: Ipv4Addr) {}

    pub fn handle_packet(packet: UdpMessage) {
        let message = DhcpMessage::from(packet.data());
        println!("{:?}", message);
    }
}

// see https://datatracker.ietf.org/doc/html/rfc2131
#[derive(Debug)]
pub struct DhcpMessage {
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

    sname: [u8; 64],
    file: [u8; 128],

    // see https://datatracker.ietf.org/doc/html/rfc2132#section-4 for a list of options
    options: Vec<u8>,
}

impl DhcpMessage {
    pub fn new(src_mac: MacAddr, host_name: String, options: Vec<u8>) -> Self {
        let empty_ipv4 = Ipv4Addr::new(0, 0, 0, 0);
        let mut sname = [0u8; 64];
        let bytes = host_name.as_bytes();
        sname[..bytes.len()].copy_from_slice(bytes);

        Self {
            op: BootpOperation::Request as u8,
            htype: HardwareType::Ethernet as u8,
            hlen: 6,
            hops: 0,
            xid: DHCP_TRANSACTION_IDENTIFIER,
            secs: 0x0,
            flags: 0x0,
            ciaddr: empty_ipv4,
            yiaddr: empty_ipv4,
            siaddr: empty_ipv4,
            giaddr: empty_ipv4,
            chaddr: src_mac,
            sname,
            file: [0u8; 128],
            options,
        }
    }

    pub fn from(payload: &[u8]) -> Self {
        let op = payload[0];
        let htype = payload[1];
        let hlen = payload[2];
        if hlen != 6 {
            panic!("Don't support DHCP where CHADDR isnt a MAC!");
        }

        let hops = payload[3];

        let xid = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
        let secs = u16::from_be_bytes([payload[8], payload[9]]);
        let flags = u16::from_be_bytes([payload[10], payload[11]]);

        let ciaddr = Ipv4Addr::new(payload[12], payload[13], payload[14], payload[15]);
        let yiaddr = Ipv4Addr::new(payload[16], payload[17], payload[18], payload[19]);
        let siaddr = Ipv4Addr::new(payload[20], payload[21], payload[22], payload[23]);
        let giaddr = Ipv4Addr::new(payload[24], payload[25], payload[26], payload[27]);

        let chaddr = MacAddr::from_bytes(&payload[28..34]);

        let mut sname = [0u8; 64];
        sname.clone_from_slice(&payload[44..108]);

        let mut file = [0u8; 128];
        file.clone_from_slice(&payload[108..236]);

        let opts = &payload[236..];

        Self {
            op,
            htype,
            hlen,
            hops,
            xid,
            secs,
            flags,
            ciaddr,
            yiaddr,
            siaddr,
            giaddr,
            chaddr,
            sname,
            file,
            options: opts.to_vec(),
        }
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let mut buf: Vec<u8> = vec![0u8; 236 + self.options.len()];
        buf[0] = self.op;
        buf[1] = self.htype;
        buf[2] = self.hlen;
        buf[3] = self.hops;
        buf[4..8].copy_from_slice(&self.xid.to_be_bytes());
        buf[8..10].copy_from_slice(&self.secs.to_be_bytes());
        buf[10..12].copy_from_slice(&self.flags.to_be_bytes());
        buf[12..16].copy_from_slice(&self.ciaddr.octets());
        buf[16..20].copy_from_slice(&self.yiaddr.octets());
        buf[20..24].copy_from_slice(&self.siaddr.octets());
        buf[24..28].copy_from_slice(&self.giaddr.octets());

        buf[28..34].copy_from_slice(&self.chaddr.octets());
        // skip from 34 -> 44 here as chaddr (MacAddr) is only 6 bytes but chaddr (in DHCP payload) must be 16 bytes:
        buf[44..108].copy_from_slice(&self.sname);
        buf[108..236].copy_from_slice(&self.file);

        buf[236..].copy_from_slice(&self.options);

        buf
    }
}

#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(u8)]
pub enum BootpOperation {
    Request = 0x1,
    Reply = 0x2,
}

pub struct DhcpOptionsBuilder {
    options: Vec<DhcpOption>,
}

impl DhcpOptionsBuilder {
    pub fn new() -> Self {
        Self {
            options: Vec::new(),
        }
    }

    pub fn message_type(mut self, t: DhcpMessageType) -> Self {
        self.options.push(DhcpOption::MessageType(t));
        self
    }

    pub fn hostname(mut self, name: impl Into<String>) -> Self {
        self.options.push(DhcpOption::Hostname(name.into()));
        self
    }

    pub fn requested_ip(mut self, ip: Ipv4Addr) -> Self {
        self.options.push(DhcpOption::RequestedIp(ip));
        self
    }

    pub fn client_identifier(mut self, htype: HardwareType, data: impl Into<Vec<u8>>) -> Self {
        self.options.push(DhcpOption::ClientIdentifier {
            htype,
            data: data.into(),
        });
        self
    }

    pub fn parameter_request_list(mut self, params: impl Into<Vec<u8>>) -> Self {
        self.options
            .push(DhcpOption::ParameterRequestList(params.into()));
        self
    }

    pub fn lease_time(mut self, secs: u32) -> Self {
        self.options.push(DhcpOption::LeaseTime(secs));
        self
    }

    pub fn option(mut self, opt: DhcpOption) -> Self {
        self.options.push(opt);
        self
    }

    pub fn build(self) -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&0x63825363u32.to_be_bytes()); // magic cookie
        for opt in &self.options {
            opt.encode(&mut buf);
        }

        if !buf.ends_with(&[255]) {
            DhcpOption::End.encode(&mut buf);
        }

        buf
    }
}

// See https://datatracker.ietf.org/doc/html/rfc2132#section-4
// sections 4-9 for valid options. Not all ate modelled here.
pub enum DhcpOption {
    MessageType(DhcpMessageType),
    RequestedIp(Ipv4Addr),
    Hostname(String),
    ParameterRequestList(Vec<u8>),
    LeaseTime(u32),
    ServerIdentifier(Ipv4Addr),
    ClientIdentifier { htype: HardwareType, data: Vec<u8> },
    Raw(u8, Vec<u8>),
    End,
}

impl DhcpOption {
    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            DhcpOption::MessageType(t) => {
                buf.extend_from_slice(&[53, 1, *t as u8]);
            }
            DhcpOption::RequestedIp(ip) => {
                buf.push(50);
                buf.push(4);
                buf.extend_from_slice(&ip.octets());
            }
            DhcpOption::Hostname(name) => {
                let bytes = name.as_bytes();
                buf.push(12);
                buf.push(bytes.len() as u8);
                buf.extend_from_slice(bytes);
            }
            DhcpOption::ParameterRequestList(params) => {
                buf.push(55);
                buf.push(params.len() as u8);
                buf.extend_from_slice(params);
            }
            DhcpOption::LeaseTime(secs) => {
                buf.push(51);
                buf.push(4);
                buf.extend_from_slice(&secs.to_be_bytes());
            }
            DhcpOption::ServerIdentifier(ip) => {
                buf.push(54);
                buf.push(4);
                buf.extend_from_slice(&ip.octets());
            }
            DhcpOption::ClientIdentifier { htype, data } => {
                buf.push(61);
                buf.push(1 + data.len() as u8);
                buf.push(*htype as u8);
                buf.extend_from_slice(data);
            }
            DhcpOption::Raw(tag, data) => {
                buf.push(*tag);
                buf.push(data.len() as u8);
                buf.extend_from_slice(data);
            }
            DhcpOption::End => buf.push(255),
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum DhcpMessageType {
    DhcpDiscover = 1,
    DhcpOffer = 2,
    DhcpRequest = 3,
    DhcpDecline = 4,
    DhcpAck = 5,
    DhcpNak = 6,
    DhcpRelease = 7,
    DhcpInform = 8,
}
