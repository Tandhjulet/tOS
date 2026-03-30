use core::net::Ipv4Addr;
use core::task::{Poll, Waker};

use alloc::{borrow::ToOwned, string::String, vec::Vec};
use alloc::{format, vec};
use num_enum::TryFromPrimitive;
use spin::Mutex;

use crate::networking::NETWORK_INFO;
use crate::networking::protocols::ethernet::HardwareType;
use crate::networking::{
    MacAddr,
    protocols::udp::{UDP, UdpMessage},
};
use crate::println;

pub const DHCP_SERVER_PORT: u16 = 67;
pub const DHCP_CLIENT_PORT: u16 = 68;
pub const DHCP_TRANSACTION_IDENTIFIER: u32 = 0x55555555;

static CURRENT_OFFER: Mutex<Option<DhcpMessage>> = Mutex::new(None);
static DHCP_WAKER: Mutex<Option<Waker>> = Mutex::new(None);
static DHCP_EVENT: Mutex<Option<DhcpEvent>> = Mutex::new(None);

static LEASE_WAKERS: Mutex<Vec<Waker>> = Mutex::new(Vec::new());

pub struct DHCP;

impl DHCP {
    pub async fn discover() {
        let mac = { NETWORK_INFO.read().mac().unwrap() };

        let options = DhcpOptions::new()
            .message_type(DhcpMessageType::DhcpDiscover)
            .client_identifier(HardwareType::Ethernet, mac.octets().to_vec())
            .parameter_request_list([
                DhcpOptionKind::SubnetMask,       // subnet mask
                DhcpOptionKind::Router,           // router
                DhcpOptionKind::DomainNameServer, // domain name server
            ]);

        let src_ip = Ipv4Addr::new(0, 0, 0, 0);
        let dst_ip = Ipv4Addr::new(255, 255, 255, 255);

        let dhcp_broadcast = DhcpMessage::new(mac, "t_os".to_owned(), options);

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

    fn post_event(event: DhcpEvent) {
        *DHCP_EVENT.lock() = Some(event);
        if let Some(waker) = DHCP_WAKER.lock().take() {
            waker.wake();
        }
    }

    pub async fn request() {
        let mac = { NETWORK_INFO.read().mac().unwrap() };

        let (yiaddr, siaddr) = {
            let lock = CURRENT_OFFER.lock();
            let offer = lock.as_ref().unwrap();
            (offer.yiaddr, offer.siaddr)
        };

        let options = DhcpOptions::new()
            .message_type(DhcpMessageType::DhcpRequest)
            .client_identifier(HardwareType::Ethernet, mac.octets().to_vec())
            .parameter_request_list([
                DhcpOptionKind::SubnetMask,       // subnet mask
                DhcpOptionKind::Router,           // router
                DhcpOptionKind::DomainNameServer, // domain name server
            ])
            .server_identifier(siaddr)
            .requested_ip(yiaddr);

        let (udp_src_ip, udp_dst_ip) = {
            // we don't own the offered ip yet - it's still 0
            let src = Ipv4Addr::new(0, 0, 0, 0);
            // requests are broadcast
            let dst = Ipv4Addr::new(255, 255, 255, 255);
            (src, dst)
        };

        let dhcp_request = DhcpMessage::new(mac, "t_os".to_owned(), options);

        UDP::send_packet(
            udp_src_ip,
            udp_dst_ip,
            DHCP_CLIENT_PORT,
            DHCP_SERVER_PORT,
            &dhcp_request.to_payload(),
        )
        .await
        .unwrap();
    }

    pub fn handle_packet(packet: UdpMessage) -> Result<(), String> {
        let message = DhcpMessage::from(packet.data())?;
        let options = &message.options;

        let Some(DhcpOption::MessageType(dhcp_type)) =
            options.find_from_tag(DhcpOptionKind::MessageType)
        else {
            return Err("Unknown DHCP type".to_owned());
        };

        match dhcp_type {
            DhcpMessageType::DhcpOffer => {
                if *dhcp_type == DhcpMessageType::DhcpOffer && CURRENT_OFFER.lock().is_some() {
                    println!("Dropping offer... already have one!");
                    return Ok(());
                }

                *CURRENT_OFFER.lock() = Some(message);

                DHCP::post_event(DhcpEvent::OfferReceived);
            }
            DhcpMessageType::DhcpAck => {
                if NETWORK_INFO.read().dhcp().is_some() {
                    return Err("Received ACK but already have lease?".to_owned());
                }

                let (offer_yi, offer_si) = {
                    let binding = CURRENT_OFFER.lock();
                    let offer = binding.as_ref().unwrap();
                    (offer.yiaddr, offer.siaddr)
                };

                if offer_yi != message.yiaddr || offer_si != message.siaddr {
                    return Err(format!(
                        "Received ACK for {} from {} but accepted offer is from {} (with ip {})",
                        message.yiaddr, message.siaddr, offer_yi, offer_si
                    ));
                }

                DHCP::post_event(DhcpEvent::Ack);
            }
            DhcpMessageType::DhcpNak => {
                if NETWORK_INFO.read().dhcp().is_some() {
                    return Err("Received NAK but already have lease?".to_owned());
                }

                DHCP::post_event(DhcpEvent::Nak);
            }

            // C -> S (unapplicable)
            DhcpMessageType::DhcpDiscover
            | DhcpMessageType::DhcpRelease
            | DhcpMessageType::DhcpDecline
            | DhcpMessageType::DhcpInform
            | DhcpMessageType::DhcpRequest => {
                println!(
                    "Received {:?} despite it being a C->S packet (?)",
                    dhcp_type
                )
            }
        }

        Ok(())
    }

    pub async fn dhcp_listener() {
        DHCP::discover().await;

        loop {
            match DhcpEventFuture.await {
                DhcpEvent::OfferReceived => {
                    DHCP::request().await;
                }
                DhcpEvent::Ack => {
                    {
                        let lock = CURRENT_OFFER.lock();
                        let dhcp = lock.as_ref().unwrap();

                        let subnet_mask =
                            match dhcp.options.find_from_tag(DhcpOptionKind::SubnetMask) {
                                Some(DhcpOption::SubnetMask(subnet_mask)) => *subnet_mask,
                                _ => Ipv4Addr::new(0, 0, 0, 0),
                            };

                        let gateway = match dhcp.options.find_from_tag(DhcpOptionKind::Router) {
                            Some(DhcpOption::Router(routers)) => *routers.first().unwrap(),
                            _ => Ipv4Addr::new(0, 0, 0, 0),
                        };

                        NETWORK_INFO.write().dhcp = Some(DhcpLease {
                            ip: dhcp.yiaddr,
                            server: dhcp.siaddr,
                            gateway,
                            client: dhcp.ciaddr,
                            subnet_mask,
                        });
                    }

                    let wakers = core::mem::take(&mut *LEASE_WAKERS.lock());
                    for waker in wakers {
                        waker.wake();
                    }

                    println!(
                        "Got a DHCP lease for ip {}!",
                        NETWORK_INFO.read().ip().unwrap()
                    );
                }
                DhcpEvent::Nak => {
                    // TODO
                    println!("Received NAK!");
                    break;
                }
            }
        }
    }
}

pub struct DhcpLease {
    ip: Ipv4Addr,
    server: Ipv4Addr,
    gateway: Ipv4Addr,
    client: Ipv4Addr,
    subnet_mask: Ipv4Addr,
    // TODO: lease duration, etc.
}

impl DhcpLease {
    pub fn ip(&self) -> &Ipv4Addr {
        &self.ip
    }

    pub fn server(&self) -> &Ipv4Addr {
        &self.server
    }

    pub fn gateway(&self) -> &Ipv4Addr {
        &self.gateway
    }

    pub fn client(&self) -> &Ipv4Addr {
        &self.client
    }

    pub fn subnet_mask(&self) -> &Ipv4Addr {
        &self.subnet_mask
    }
}

pub struct EnsureDHCPLease;

impl Future for EnsureDHCPLease {
    type Output = ();

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> Poll<Self::Output> {
        if let Some(_) = NETWORK_INFO.read().dhcp() {
            return Poll::Ready(());
        }

        LEASE_WAKERS.lock().push(cx.waker().clone());

        if let Some(_) = NETWORK_INFO.read().ip() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

pub enum DhcpEvent {
    OfferReceived,
    Ack,
    Nak,
}

struct DhcpEventFuture;

impl Future for DhcpEventFuture {
    type Output = DhcpEvent;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        if let Some(event) = DHCP_EVENT.lock().take() {
            return Poll::Ready(event);
        }

        *DHCP_WAKER.lock() = Some(cx.waker().clone());

        if let Some(event) = DHCP_EVENT.lock().take() {
            Poll::Ready(event)
        } else {
            Poll::Pending
        }
    }
}

// see https://datatracker.ietf.org/doc/html/rfc2131
#[derive(Debug)]
pub struct DhcpMessage {
    op: BootpOperation,
    htype: HardwareType,
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
    options: DhcpOptions,
}

impl DhcpMessage {
    pub fn new(src_mac: MacAddr, host_name: String, options: DhcpOptions) -> Self {
        let empty_ipv4 = Ipv4Addr::new(0, 0, 0, 0);
        let mut sname = [0u8; 64];
        let bytes = host_name.as_bytes();
        sname[..bytes.len()].copy_from_slice(bytes);

        Self {
            op: BootpOperation::Request,
            htype: HardwareType::Ethernet,
            hlen: 6,
            hops: 0,
            xid: DHCP_TRANSACTION_IDENTIFIER,
            secs: 0x0,
            flags: 0x0,
            ciaddr: empty_ipv4,
            giaddr: empty_ipv4,
            yiaddr: empty_ipv4,
            siaddr: empty_ipv4,
            chaddr: src_mac,
            sname,
            file: [0u8; 128],
            options,
        }
    }

    pub fn from(payload: &[u8]) -> Result<Self, String> {
        let op = BootpOperation::try_from(payload[0])
            .map_err(|err| format!("failed to map {} into a BOOTP operation!", err.number))?;

        let htype = HardwareType::try_from(payload[1] as u16)
            .map_err(|err| format!("failed to map {} into a hardware type!", err.number))?;

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

        let options = DhcpOptions::parse(&payload[236..]);

        Ok(Self {
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
            options,
        })
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let opts = &self.options.build();

        let mut buf: Vec<u8> = vec![0u8; 236 + opts.len()];
        buf[0] = self.op as u8;
        buf[1] = self.htype as u8;
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

        buf[236..].copy_from_slice(opts);

        buf
    }
}

#[derive(Debug, Clone, Copy, TryFromPrimitive)]
#[repr(u8)]
pub enum BootpOperation {
    Request = 0x1,
    Reply = 0x2,
}

#[derive(Debug)]
pub struct DhcpOptions {
    options: Vec<DhcpOption>,
}

impl DhcpOptions {
    pub fn new() -> Self {
        Self {
            options: Vec::new(),
        }
    }

    pub fn message_type(mut self, t: DhcpMessageType) -> Self {
        self.options.push(DhcpOption::MessageType(t));
        self
    }

    pub fn find_from_tag(&self, tag: DhcpOptionKind) -> Option<&DhcpOption> {
        self.options.iter().find(|opt| opt.kind() == tag)
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

    pub fn parameter_request_list(mut self, params: impl Into<Vec<DhcpOptionKind>>) -> Self {
        self.options
            .push(DhcpOption::ParameterRequestList(params.into()));
        self
    }

    pub fn lease_time(mut self, secs: u32) -> Self {
        self.options.push(DhcpOption::LeaseTime(secs));
        self
    }

    pub fn server_identifier(mut self, identifier: Ipv4Addr) -> Self {
        self.options.push(DhcpOption::ServerIdentifier(identifier));
        self
    }

    pub fn option(mut self, opt: DhcpOption) -> Self {
        self.options.push(opt);
        self
    }

    pub fn build(&self) -> Vec<u8> {
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

    pub fn parse(mut buf: &[u8]) -> Self {
        let mut options: Vec<DhcpOption> = Vec::new();
        buf = &buf[4..]; // skip magic cookie

        while !buf.is_empty() {
            match DhcpOption::decode(buf) {
                Some((opt, consumed)) => {
                    let done = matches!(opt, DhcpOption::End);

                    buf = &buf[consumed..];
                    options.push(opt);

                    if done {
                        break;
                    }
                }
                None => {
                    println!("Failed to decode some opt! Failing...");
                    break;
                }
            }
        }

        Self { options }
    }
}

// See https://datatracker.ietf.org/doc/html/rfc2132#section-4
// sections 4-9 for valid options. Not all ate modelled here.
#[derive(Debug)]
pub enum DhcpOption {
    SubnetMask(Ipv4Addr), // FIXME: should this be an ipv4 addr?
    Router(Vec<Ipv4Addr>),
    DomainNameServer(Vec<Ipv4Addr>),
    MessageType(DhcpMessageType),
    RequestedIp(Ipv4Addr),
    Hostname(String),
    ParameterRequestList(Vec<DhcpOptionKind>),
    LeaseTime(u32),
    ServerIdentifier(Ipv4Addr),
    ClientIdentifier { htype: HardwareType, data: Vec<u8> },
    Raw { tag: u8, data: Vec<u8> },
    End,
}

#[derive(PartialEq, Eq, Debug, TryFromPrimitive, Clone, Copy)]
#[repr(u8)]
pub enum DhcpOptionKind {
    SubnetMask = 1,
    Router = 3,
    DomainNameServer = 6,
    MessageType = 53,
    RequestedIp = 50,
    Hostname = 12,
    ParameterRequestList = 55,
    LeaseTime = 51,
    ServerIdentifier = 54,
    ClientIdentifier = 61,
    End = 255,
    Unknown = 0,
}

impl DhcpOption {
    pub fn kind(&self) -> DhcpOptionKind {
        match self {
            DhcpOption::MessageType(_) => DhcpOptionKind::MessageType,
            DhcpOption::RequestedIp(_) => DhcpOptionKind::RequestedIp,
            DhcpOption::Hostname(_) => DhcpOptionKind::Hostname,
            DhcpOption::ParameterRequestList(_) => DhcpOptionKind::ParameterRequestList,
            DhcpOption::LeaseTime(_) => DhcpOptionKind::LeaseTime,
            DhcpOption::ServerIdentifier(_) => DhcpOptionKind::ServerIdentifier,
            DhcpOption::ClientIdentifier { .. } => DhcpOptionKind::ClientIdentifier,
            DhcpOption::End => DhcpOptionKind::End,
            DhcpOption::Raw { .. } => DhcpOptionKind::Unknown,
            DhcpOption::SubnetMask(_) => DhcpOptionKind::SubnetMask,
            DhcpOption::Router(_) => DhcpOptionKind::Router,
            DhcpOption::DomainNameServer(_) => DhcpOptionKind::DomainNameServer,
        }
    }

    fn write_tag(&self, buf: &mut Vec<u8>, data: &[u8]) {
        buf.push(self.kind() as u8);
        buf.push(data.len() as u8);
        buf.extend_from_slice(data);
    }

    pub fn encode(&self, buf: &mut Vec<u8>) {
        match self {
            DhcpOption::SubnetMask(mask) => self.write_tag(buf, &mask.octets()),
            DhcpOption::DomainNameServer(ipv4_addrs) | DhcpOption::Router(ipv4_addrs) => {
                let addrs: Vec<u8> = ipv4_addrs.iter().flat_map(|ip| ip.octets()).collect();
                self.write_tag(buf, &addrs);
            }
            DhcpOption::MessageType(t) => self.write_tag(buf, &[*t as u8]),
            DhcpOption::RequestedIp(ip) => self.write_tag(buf, &ip.octets()),
            DhcpOption::Hostname(name) => self.write_tag(buf, name.as_bytes()),
            DhcpOption::ParameterRequestList(params) => {
                let params: Vec<u8> = params.iter().map(|kind| (*kind) as u8).collect();
                self.write_tag(buf, &params);
            }
            DhcpOption::LeaseTime(secs) => self.write_tag(buf, &secs.to_be_bytes()),
            DhcpOption::ServerIdentifier(ip) => self.write_tag(buf, &ip.octets()),
            DhcpOption::ClientIdentifier { htype, data } => {
                buf.push(self.kind() as u8);
                buf.push(1 + data.len() as u8);
                buf.push(*htype as u8);
                buf.extend_from_slice(data);
            }
            DhcpOption::Raw { tag, data } => {
                buf.push(*tag);
                buf.push(data.len() as u8);
                buf.extend_from_slice(data);
            }
            DhcpOption::End => {
                buf.push(0xFF);
            }
        }
    }

    pub fn decode(buf: &[u8]) -> Option<(DhcpOption, usize)> {
        let raw_tag = *buf.first()?;
        let Ok(tag) = DhcpOptionKind::try_from_primitive(raw_tag) else {
            println!("Received unknown DHCP tag {}!", raw_tag);
            return None;
        };

        if tag == DhcpOptionKind::End {
            return Some((DhcpOption::End, 1));
        }

        let len = *buf.get(1)? as usize;
        let data = buf.get(2..2 + len)?;

        let opt = match tag {
            DhcpOptionKind::MessageType => {
                DhcpOption::MessageType(DhcpMessageType::try_from(*data.first()?).ok()?)
            }
            DhcpOptionKind::RequestedIp => {
                DhcpOption::RequestedIp(Ipv4Addr::new(data[0], data[1], data[2], data[3]))
            }
            DhcpOptionKind::Hostname => {
                DhcpOption::Hostname(String::from_utf8(data.to_vec()).ok()?)
            }
            DhcpOptionKind::ParameterRequestList => {
                let reqs: Vec<DhcpOptionKind> = data
                    .to_vec()
                    .iter()
                    .filter_map(|v| DhcpOptionKind::try_from(*v).ok())
                    .collect();
                DhcpOption::ParameterRequestList(reqs)
            }
            DhcpOptionKind::LeaseTime => {
                DhcpOption::LeaseTime(u32::from_be_bytes(data.try_into().ok()?))
            }
            DhcpOptionKind::ServerIdentifier => {
                DhcpOption::ServerIdentifier(Ipv4Addr::new(data[0], data[1], data[2], data[3]))
            }
            DhcpOptionKind::ClientIdentifier => DhcpOption::ClientIdentifier {
                htype: HardwareType::try_from(data[0] as u16).ok()?,
                data: data[1..].to_vec(),
            },
            DhcpOptionKind::SubnetMask => {
                DhcpOption::SubnetMask(Ipv4Addr::new(data[0], data[1], data[2], data[3]))
            }
            DhcpOptionKind::Router => {
                let routers = data
                    .chunks(4)
                    .map(|chunk| Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]))
                    .collect();

                DhcpOption::Router(routers)
            }
            DhcpOptionKind::DomainNameServer => {
                let dn_servers = data
                    .chunks(4)
                    .map(|chunk| Ipv4Addr::new(chunk[0], chunk[1], chunk[2], chunk[3]))
                    .collect();

                DhcpOption::Router(dn_servers)
            }
            DhcpOptionKind::End | DhcpOptionKind::Unknown => {
                panic!("This should never occur!")
            }
        };

        Some((opt, 2 + len))
    }
}

#[derive(Debug, Clone, Copy, TryFromPrimitive, PartialEq, Eq)]
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
