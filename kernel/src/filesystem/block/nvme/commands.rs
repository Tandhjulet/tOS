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
