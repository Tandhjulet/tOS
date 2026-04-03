pub mod nvme;

pub trait StorageDevice {
    fn read();
    fn write();
}
