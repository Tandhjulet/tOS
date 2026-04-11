#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kernel::test_runner)]
#![reexport_test_harness_main = "test_main"]

use bootloader_api::{BootInfo, BootloaderConfig, config::Mapping, entry_point};
use kernel::{
    allocator, init_logger,
    io::net::{network_rx_task, network_tx_task},
    println, serial_println,
    sys::{
        self,
        interrupts::{self},
        task::{Task, executor::Executor, keyboard},
    },
};
use log::{error, info};

extern crate alloc;

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    sys::gdt::init();

    init_framebuffer(boot_info);
    allocator::init(boot_info).expect("heap initialization failed");

    if let Err(msg) = interrupts::init() {
        error!("INTERRUPT: {}", msg);
    }

    x86_64::instructions::interrupts::enable();

    // if let Err(msg) = Acpi::try_init(boot_info.rsdp_addr) {
    //     error!("ACPI: {}", msg);
    // }

    // if let Err(msg) = interrupts::try_init_apic() {
    //     error!("APIC: {}", msg);
    // }

    // pci::init();

    // networking::init();
    // filesystem::init();

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

fn init_framebuffer(boot_info: *mut BootInfo) {
    // SAFETY: never access boot_info.framebuffer after this method returns
    let buf: &'static mut [u8] = unsafe {
        let fb = (*boot_info).framebuffer.as_mut().unwrap();
        let buf = fb.buffer_mut();
        serial_println!("FB addr: {:#x} len: {:#x}", buf.as_ptr() as u64, buf.len());
        core::slice::from_raw_parts_mut(buf.as_mut_ptr(), buf.len())
    };
    let info = unsafe { (*boot_info).framebuffer.as_ref().unwrap().info() };

    init_logger(buf, info);
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
