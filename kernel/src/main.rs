#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader_api::{BootInfo, BootloaderConfig, config::Mapping, entry_point};
use kernel::{
    filesystem, init_logger, interrupts,
    networking::{network_rx_task, network_tx_task},
    task::{Task, executor::Executor, keyboard},
};

extern crate alloc;

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    let frame_buffer = boot_info.framebuffer.as_mut().unwrap();
    let info = frame_buffer.info();
    let buf = frame_buffer.buffer_mut();
    init_logger(buf, info);

    kernel::init();
    // allocator::init(boot_info).expect("heap initialization failed");

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

#[cfg(not(test))]
use core::panic::PanicInfo;

/// This function is called on panic.
#[cfg(not(test))]
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use kernel::println;

    println!("{}", info);
    kernel::hlt_loop();
}
