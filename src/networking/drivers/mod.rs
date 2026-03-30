use alloc::vec::Vec;
use x86_64::{instructions::interrupts::without_interrupts, structures::idt::InterruptStackFrame};

use crate::{
    interrupts::{MIN_INTERRUPT, PICS},
    networking::{MacAddr, NETWORK_DRIVER, RX_WAKER},
};

pub mod e1000;

pub trait NetworkDriver: Send {
    fn start(&mut self);
    fn get_mac_addr(&self) -> &MacAddr;
    fn is_up(&mut self) -> bool;
    fn prepare_transmit(&mut self, data: &[u8]);
    fn transmit(&mut self);
    fn handle_interrupt(&mut self, stack_frame: InterruptStackFrame);
    fn get_interrupt_line(&self) -> u8;
}

impl dyn NetworkDriver {
    extern "x86-interrupt" fn fire(stack_frame: InterruptStackFrame) {
        let irq_line = {
            let mut driver = NETWORK_DRIVER.lock();
            let driver = driver.as_mut().unwrap();

            driver.handle_interrupt(stack_frame);
            driver.get_interrupt_line()
        };

        unsafe {
            let remapped_line = irq_line + (MIN_INTERRUPT as u8);
            PICS.lock().notify_end_of_interrupt(remapped_line);
        }

        RX_WAKER.wake();
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
