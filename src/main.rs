#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(tOS::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{BootInfo, entry_point};
use core::panic::PanicInfo;
use tOS::{memory::BootInfoFrameAllocator, println};
use x86_64::structures::paging::Page;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    use tOS::memory;
    use x86_64::VirtAddr;

    println!("Hello World{}", "!");

    tOS::init();

    #[cfg(test)]
    test_main();

    println!("It did not crash!");
    tOS::hlt_loop();
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
