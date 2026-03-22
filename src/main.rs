#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(tOS::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;
use tOS::{
    allocator,
    networking::{self, protocols::arp::Arp},
    println,
    task::{Task, executor::Executor, keyboard},
};

extern crate alloc;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    tOS::init();
    allocator::init(&boot_info).expect("heap initialization failed");

    networking::init();
    Arp::send().unwrap();

    let mut executor = Executor::new();
    executor.spawn(Task::new(keyboard::print_keypresses()));
    executor.run();
}

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("{}", info);
    tOS::hlt_loop();
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    tOS::test_panic_handler(info)
}

#[test_case]
fn trivial_assertion() {
    assert_eq!(1, 1);
}
