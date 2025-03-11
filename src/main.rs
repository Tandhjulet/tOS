#![no_std] // the std won't be available in the os env
#![no_main] // disable all rust entry points
#![feature(custom_test_frameworks)]
#![test_runner(t_os::tests::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    println!("Hello World{}", "!");

    t_os::init();

    x86_64::instructions::interrupts::int3();

    #[cfg(test)]
    test_main();

    loop {}
}

// Called on panic
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    serial_println!("{}", info.message().as_str().unwrap_or("Panicked!"));
    loop {}
}

#[cfg(test)]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    t_os::tests::panic_handler(info)
}

pub mod serial;
pub mod vga_buffer;
