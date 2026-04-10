use alloc::vec::Vec;
use x86_64::instructions::interrupts::without_interrupts;

use crate::{
    interrupts::MIN_INTERRUPT,
    io::net::{MacAddr, NETWORK_DRIVER, RX_WAKER},
    sys::interrupts::{INTERRUPT_CONTROLLER, IrqResult},
};

pub mod e1000;

pub trait NetworkDriver: Send {
    fn start(&mut self);
    fn get_mac_addr(&self) -> &MacAddr;
    fn is_up(&mut self) -> bool;
    fn prepare_transmit(&mut self, data: &[u8]);
    fn transmit(&mut self);
    fn handle_interrupt(&mut self);
    fn get_interrupt_line(&self) -> u8;
}

impl dyn NetworkDriver {
    fn fire() -> IrqResult {
        let irq_line = {
            let mut driver = NETWORK_DRIVER.lock();
            let driver = driver.as_mut().unwrap();

            driver.handle_interrupt();
            driver.get_interrupt_line()
        };

        let remapped_line = irq_line + (MIN_INTERRUPT as u8);
        INTERRUPT_CONTROLLER.eoi(remapped_line);

        RX_WAKER.wake();
        IrqResult::EoiSent
    }

    pub fn send_packet(data: Vec<u8>) {
        {
            NETWORK_DRIVER
                .lock()
                .as_mut()
                .unwrap()
                .prepare_transmit(&data);
        }

        without_interrupts(|| {
            NETWORK_DRIVER.lock().as_mut().unwrap().transmit();
        })
    }
}
