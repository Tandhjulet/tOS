///
/// NVMe Documentation:
/// - Base specification: https://nvmexpress.org/wp-content/uploads/NVMe-NVM-Express-2.0a-2021.07.26-Ratified.pdf
/// - NVMe over PCIe specification: https://nvmexpress.org/wp-content/uploads/NVM-Express-NVMe-over-PCIe-Transport-Specification-Revision-1.3-2025.08.01-Ratified.pdf
/// - NVM Host Controller Interface (has good overview over commands): https://www.nvmexpress.org/wp-content/uploads/NVM-Express-1_1a.pdf
///
pub const IO_QUEUES: u16 = 2;

pub const CAP: u32 = 0x0;
pub const VS: u32 = 0x08;
pub const INTMS: u32 = 0x0C;
pub const INTMC: u32 = 0x10;
pub const CC: u32 = 0x14;
pub const CSTS: u32 = 0x1C;
pub const AQA: u32 = 0x24;
pub const ASQ: u32 = 0x28;
pub const ACQ: u32 = 0x30;

// See figure 138 for at list of operations
pub mod op {
    pub const IDENTIFY: u32 = 0x06;
    pub const SET_FEATURES: u32 = 0x09;
    pub const GET_FEATURES: u32 = 0x0A;
    pub const DEL_SUBQ: u32 = 0x0;
    pub const CRT_SUBQ: u32 = 0x01;
    pub const DEL_CMPQ: u32 = 0x04;
    pub const CRT_CMPQ: u32 = 0x05;

    // For a list of Identify CNS values and reference sections, view figure 273
    pub mod identify {
        pub const CNS_NAMESPACE: u32 = 0x0;
        pub const CNS_CONTROLLER: u32 = 0x1;
        pub const CNS_SPECIFIC_NS: u32 = 0x5;
        pub const CNS_SPECIFIC_CTRLR: u32 = 0x6;
        pub const CNS_ACTIVE_NS_CMD_SET: u32 = 0x7;
        pub const CNS_NAMESPACE_INDEPENDENT: u32 = 0x8;
        pub const CNS_CMD_SET: u32 = 0x1C;
    }

    pub mod features {
        pub const FID_NUM_QUEUES: u32 = 0x07;
        pub const FID_SET_PROFILE: u32 = 0x19;
        pub const FID_INT_VEC_CONF: u32 = 0x09;
    }
}
