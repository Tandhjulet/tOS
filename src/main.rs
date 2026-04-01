#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(tOS::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader::{BootInfo, entry_point};
use core::{net::Ipv4Addr, panic::PanicInfo};
use tOS::{
    allocator, filesystem, interrupts,
    networking::{
        self, network_rx_task, network_tx_task,
        protocols::{dhcp::DHCP, tcp::TcpConnection},
    },
    println,
    task::{Task, executor::Executor, keyboard},
};

extern crate alloc;

entry_point!(kernel_main);

fn kernel_main(boot_info: &'static BootInfo) -> ! {
    let physical_memory_offset = boot_info.physical_memory_offset;
    println!("physical memory offset: {:#x}", physical_memory_offset);

    tOS::init();
    allocator::init(&boot_info).expect("heap initialization failed");

    // networking::init();
    filesystem::init();

    interrupts::load_idt();

    let mut executor = Executor::new();
    executor.spawn(Task::new(network_rx_task()));
    executor.spawn(Task::new(network_tx_task()));
    executor.spawn(Task::new(kernel_main_task()));
    executor.spawn(Task::new(keyboard::print_keypresses()));
    executor.run();
}

async fn kernel_main_task() {
    // DHCP::discover().await.unwrap();

    // let dst = Ipv4Addr::new(1, 1, 1, 1);
    // let mut tcp = TcpConnection::new(dst, 1234, 80).await;
    // tcp.open().await.unwrap();

    // tcp.close().await.unwrap();
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
