pub mod nvme;

pub trait BlockDevice {
    fn read();
    fn write();
}
