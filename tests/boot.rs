#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(t_os::tests::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    test_main();

    loop {}
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    t_os::tests::panic_handler(info)
}

mod tests {
    use t_os::println;

    #[test_case]
    fn test_println() {
        println!("test_println output");
    }
}
