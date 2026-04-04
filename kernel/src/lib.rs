#![no_std]
#![cfg_attr(test, no_main)]
#![feature(custom_test_frameworks)]
#![feature(abi_x86_interrupt)]
#![test_runner(crate::test_runner)]
#![reexport_test_harness_main = "test_main"]

pub mod allocator;
pub mod filesystem;
pub mod helpers;
pub mod io;
pub mod sys;

use core::panic::PanicInfo;

use bootloader_api::info::FrameBufferInfo;

use crate::{io::logger::LockedLogger, sys::interrupts};

extern crate alloc;

pub fn init() {
    sys::gdt::init();

    interrupts::init_idt();
    interrupts::init_pics();

    x86_64::instructions::interrupts::enable();
}

pub fn init_logger(buf: &'static mut [u8], info: FrameBufferInfo) {
    let logger = io::logger::LOGGER.get_or_init(move || LockedLogger::new(buf, info));
    log::set_logger(logger).expect("logger already set!");
    log::set_max_level(log::LevelFilter::Trace);
}

pub trait Testable {
    fn run(&self) -> ();
}

impl<T> Testable for T
where
    T: Fn(),
{
    fn run(&self) {
        serial_print!("{}...\t", core::any::type_name::<T>());
        self();
        serial_println!("[ok]");
    }
}

pub fn test_runner(tests: &[&dyn Testable]) {
    serial_println!("Running {} tests", tests.len());
    for test in tests {
        test.run();
    }
    exit_qemu(QemuExitCode::Success);
}

pub fn test_panic_handler(info: &PanicInfo) -> ! {
    serial_println!("[failed]\n");
    serial_println!("Error: {}\n", info);
    exit_qemu(QemuExitCode::Failed);
    hlt_loop();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum QemuExitCode {
    Success = 0x10,
    Failed = 0x11,
}

pub fn exit_qemu(exit_code: QemuExitCode) {
    use x86_64::instructions::port::Port;

    unsafe {
        let mut port = Port::new(0xf4);
        port.write(exit_code as u32);
    }
}

pub fn hlt_loop() -> ! {
    loop {
        x86_64::instructions::hlt();
    }
}
