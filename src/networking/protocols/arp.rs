use core::net::Ipv4Addr;

use crate::networking::{EtherType, MacAddr, NETWORK_DRIVER};

pub struct ArpMessage {
    pub ethertype: EtherType,
    pub operation: Operation,

    pub src_hw_addr: MacAddr,
    pub src_pc_addr: Ipv4Addr,

    pub dst_hw_addr: MacAddr,
    pub dst_pc_addr: Ipv4Addr,
}

impl ArpMessage {
    pub fn new(dst_hw_addr: MacAddr, dst_pc_addr: Ipv4Addr) -> Self {
        // hardcode IP until we get one
        const SRC_IP: Ipv4Addr = Ipv4Addr::new(10, 2, 2, 3);

        let lock = NETWORK_DRIVER.lock();
        let mac = lock.as_ref().unwrap().get_mac_addr();

        Self {
            ethertype: EtherType::ARP,
            operation: Operation::ArpRequest,
            src_hw_addr: *mac,
            src_pc_addr: SRC_IP,
            dst_hw_addr,
            dst_pc_addr,
        }
    }
}

pub enum Operation {
    ArpRequest = 0x1,
    ArpResponse = 0x2,
    RarpRequest = 0x3,
    RarpResponse = 0x4,
}
