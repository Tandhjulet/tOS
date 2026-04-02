use alloc::sync::Arc;
use spin::Mutex;

use crate::{
    allocator::mmio,
    filesystem::drivers::StorageDevice,
    pci::{PciDevice, bar::BarKind},
    println,
};

pub struct NVMe {}

impl NVMe {
    pub fn new(guard: Arc<Mutex<PciDevice>>) -> Self {
        let binding = guard.lock();
        let Some(bar0) = PciDevice::get_bar(&binding, 0) else {
            panic!("Could not find BAR0 for NVMe!");
        };

        // TODO: move this into bar mod
        if let BarKind::Mem { addr, .. } = bar0.kind() {
            let virt_addr = {
                let phys_addr = addr.phys();

                let size = bar0.size() as u64;
                mmio::map_mmio(phys_addr, size)
            };

            bar0.set_virt_addr(virt_addr);
        }

        println!("bar0: {:?}", bar0);

        Self {}
    }
}

impl StorageDevice for NVMe {
    fn read() {
        todo!()
    }

    fn write() {
        todo!()
    }
}
