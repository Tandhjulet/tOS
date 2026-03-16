#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(tOS::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

use alloc::{boxed::Box, vec::Vec};
use bootloader::{BootInfo, entry_point};
use tOS::allocator::{self, HEAP_SIZE};

extern crate alloc;

entry_point!(main);

fn main(boot_info: &'static BootInfo) -> ! {
    tOS::init();
    allocator::init(&boot_info).expect("heap initialization failed");

    test_main();
    loop {}
}

#[test_case]
fn simple_alloc() {
    let hv1 = Box::new(41);
    let hv2 = Box::new(13);
    assert_eq!(*hv1, 41);
    assert_eq!(*hv2, 13);
}

#[test_case]
fn large_vec() {
    let n = 1000;
    let mut vec = Vec::new();
    for i in 0..n {
        vec.push(i);
    }

    assert_eq!(vec.iter().sum::<u64>(), (n - 1) * n / 2);
}

#[test_case]
fn many_boxes() {
    for i in 0..HEAP_SIZE {
        let x = Box::new(i);
        assert_eq!(*x, i);
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    tOS::test_panic_handler(info);
}
