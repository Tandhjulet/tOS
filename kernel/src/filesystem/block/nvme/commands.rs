use crate::filesystem::block::nvme::{queue::SQEntry, spec};

pub trait NvmeCommand {
    const OPCODE: u32;
    fn configure(self, entry: &mut SQEntry);
}

pub struct IdentifyCommand {
    pub cns: u32,
    pub nsid: u32,
    pub cdw11: u32,
    pub prp: u64,
}

impl NvmeCommand for IdentifyCommand {
    const OPCODE: u32 = spec::op::IDENTIFY;

    fn configure(self, entry: &mut SQEntry) {
        entry.prp1 = self.prp;
        entry.cdw10 = self.cns;
        entry.cdw11 = self.cdw11;
        entry.nsid = self.nsid;
    }
}

pub struct SetFeaturesCommand {
    pub fid: u32,
    pub prp: u64,
    pub cdw11: u32,
}

impl NvmeCommand for SetFeaturesCommand {
    const OPCODE: u32 = spec::op::SET_FEATURES;

    fn configure(self, entry: &mut SQEntry) {
        entry.prp1 = self.prp;
        entry.cdw10 = self.fid;
        entry.cdw11 = self.cdw11;
    }
}

pub struct CreateSubmissionQueueCommand {
    pub prp: u64,
    pub cdw10: u32,
    pub cdw11: u32,
}

impl NvmeCommand for CreateSubmissionQueueCommand {
    const OPCODE: u32 = spec::op::CRT_SUBQ;

    fn configure(self, entry: &mut SQEntry) {
        entry.prp1 = self.prp;
        entry.cdw10 = self.cdw10;
        entry.cdw11 = self.cdw11;
    }
}

pub struct CreateCompletionQueueCommand {
    pub prp: u64,
    pub cdw10: u32,
    pub cdw11: u32,
}

impl NvmeCommand for CreateCompletionQueueCommand {
    const OPCODE: u32 = spec::op::CRT_CMPQ;

    fn configure(self, entry: &mut SQEntry) {
        entry.prp1 = self.prp;
        entry.cdw10 = self.cdw10;
        entry.cdw11 = self.cdw11;
    }
}

pub struct ReadCommand {
    pub nsid: u32,
    pub prp: u64,
    pub slba: u64,
    pub cdw12: u32,
    pub dsm: u8,
}

impl NvmeCommand for ReadCommand {
    const OPCODE: u32 = spec::op::READ;

    fn configure(self, entry: &mut SQEntry) {
        entry.nsid = self.nsid;
        entry.prp1 = self.prp;
        entry.cdw10 = self.slba as u32;
        entry.cdw11 = (self.slba >> 32) as u32;
        entry.cdw12 = self.cdw12;
        entry.cdw13 = self.dsm as u32;
    }
}

pub struct WriteCommand {
    pub nsid: u32,
    pub prp: u64,
    pub slba: u64,
    pub cdw12: u32,
    pub dsm: u8,
}

impl NvmeCommand for WriteCommand {
    const OPCODE: u32 = spec::op::WRITE;

    fn configure(self, entry: &mut SQEntry) {
        entry.nsid = self.nsid;
        entry.prp1 = self.prp;
        entry.cdw10 = self.slba as u32;
        entry.cdw11 = (self.slba >> 32) as u32;
        entry.cdw12 = self.cdw12;
        entry.cdw13 = self.dsm as u32;
    }
}

pub struct FlushCommand {
    pub nsid: u32,
}

impl NvmeCommand for FlushCommand {
    const OPCODE: u32 = spec::op::FLUSH;

    fn configure(self, entry: &mut SQEntry) {
        entry.nsid = self.nsid;
    }
}

pub struct ControllerConfig(pub u32);

impl ControllerConfig {
    pub fn raw(&self) -> u32 {
        self.0
    }

    pub fn set_raw(&mut self, raw: u32) -> &mut Self {
        self.0 = raw;
        self
    }

    pub fn set_css_from_cap(&mut self, cap: &ControllerCap) -> &mut Self {
        let mut css = 0b000u8;
        if cap.css_none() {
            css = 0b111;
        } else if cap.css_some() {
            css = 0b110;
        }
        self.set_css(css);
        self
    }

    pub fn set_css(&mut self, css: u8) -> &mut Self {
        self.0 = self.0 & !(0x7 << 4) | ((css as u32 & 0x7) << 4);
        self
    }

    pub fn css(&self) -> u8 {
        ((self.0 >> 4) & 0x7) as u8
    }

    pub fn set_enabled(&mut self, en: bool) -> &mut Self {
        self.0 = (self.0 & !0x1) | (en as u32);
        self
    }

    pub fn set_iosqes(&mut self, iosqes: u32) -> &mut Self {
        self.0 = (self.0 & !(0xF << 16)) | ((iosqes & 0xF) << 16);
        self
    }

    pub fn set_iocqes(&mut self, iocqes: u32) -> &mut Self {
        self.0 = (self.0 & !(0xF << 20)) | ((iocqes & 0xF) << 20);
        self
    }

    pub fn set_ams(&mut self, ams: AmsType) -> &mut Self {
        self.0 = (self.0 & !(0b111 << 11)) | ((ams as u32 & 0b111) << 11);
        self
    }

    pub fn set_mps(&mut self, mps: u32) -> &mut Self {
        self.0 = (self.0 & !(0xF << 7)) | ((mps & 0xF) << 7);
        self
    }
}

#[repr(u8)]
pub enum AmsType {
    RoundRobin = 0b000,
    WeightedRoundRobin = 0b001,
    Vendor = 0b111,
}

pub struct ControllerCap(pub u64);

impl ControllerCap {
    pub fn dstrd(&self) -> u8 {
        ((self.0 >> 32) & 0xF) as u8
    }

    pub fn mqes(&self) -> u16 {
        (self.0 & 0xFFFF) as u16
    }

    pub fn to(&self) -> u8 {
        ((self.0 >> 24) & 0xFF) as u8
    }

    pub fn mpsmin(&self) -> u8 {
        ((self.0 >> 48) & 0xF) as u8
    }

    ///
    /// Command Sets Supported (CSS)
    ///
    pub fn css(&self) -> u8 {
        ((self.0 >> 37) & 0xFF) as u8
    }

    pub fn css_nvm(&self) -> bool {
        self.css() & 0x1 == 1
    }

    pub fn css_none(&self) -> bool {
        (self.css() >> 7) & 0x1 == 1
    }

    pub fn css_some(&self) -> bool {
        (self.css() >> 6) & 0x1 == 1
    }
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyController {
    pub vid: u16,
    pub ssvid: u16,
    pub sn: [u8; 20],
    pub mn: [u8; 40],
    pub fr: [u8; 8],
    pub rab: u8,
    pub ieee_oui: [u8; 3],
    pub cmic: u8,
    pub mdts: u8,
    pub ctlr_id: u16,
    pub ver: [u8; 3],
    // see Figure 275 for the rest of the fields... there's a lot
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct IdentifyCommandSet {
    pub iocsc: [u64; 512],
}

impl IdentifyCommandSet {
    pub fn is_nvm_supported(cmd_set: u64) -> bool {
        cmd_set & 0x1 == 1
    }

    pub fn is_kv_supported(cmd_set: u64) -> bool {
        cmd_set & 0x2 == 1
    }

    pub fn is_zns_supported(cmd_set: u64) -> bool {
        cmd_set & 0x4 == 1
    }

    pub fn first_valid(&self) -> Option<usize> {
        self.iocsc.iter().position(|&e| Self::is_nvm_supported(e))
    }

    pub fn csi_iter(&self, idx: usize) -> impl Iterator<Item = u8> {
        let entry = self.iocsc[idx];
        (0u8..3).filter(move |&bit| entry & (1 << bit) == 1)
    }
}
