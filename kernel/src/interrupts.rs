use crate::{gdt, hlt_loop, println};
use bootloader_api::BootInfo;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

pub const MIN_INTERRUPT: usize = 32;
pub const PIC_1_OFFSET: u8 = MIN_INTERRUPT as u8;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: spin::Mutex<ChainedPics> =
    spin::Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

impl InterruptIndex {
    fn as_u8(self) -> u8 {
        self as u8
    }

    fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

// FIXME: dont use a Mutex here... instead, use something like:
// static HANDLERS: Mutex<[Option<fn()>; 256]> = Mutex::new([None; 256]);
// and then generate 256 functions as unique handlers for each IRQ number using a macro
pub static IDT: Mutex<InterruptDescriptorTable> = Mutex::new(InterruptDescriptorTable::new());

pub fn init_pics() {
    unsafe {
        PICS.lock().initialize();

        let [master, slave] = PICS.lock().read_masks();
        PICS.lock().write_masks(master & !(1u8 << 2), slave);
    }
}

pub fn init_idt() {
    let mut idt = IDT.lock();
    idt.breakpoint.set_handler_fn(breakpoint_handler);
    unsafe {
        idt.double_fault
            .set_handler_fn(double_fault_handler)
            .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
    }
    idt.page_fault.set_handler_fn(page_fault_handler);

    // FIXME: don't hardcode... create a registrar
    idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
    idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);

    // clear lock and load
    drop(idt);
    load_idt();
}

pub fn load_idt() {
    let idt = IDT.lock();
    let idt_static: &'static InterruptDescriptorTable = unsafe { &*(&*idt as *const _) };
    idt_static.load();
}

pub fn setup_msi(_boot_info: &'static BootInfo) {}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    use x86_64::registers::control::Cr2;

    println!("PAGE FAULT");
    println!("Addr: {:?}", Cr2::read());
    println!("Err: {:?}", error_code);
    println!("{:#?}", stack_frame);
    hlt_loop();
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Timer.as_u8());
    }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::task::keyboard::add_scancode(scancode);

    unsafe {
        PICS.lock()
            .notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8());
    }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    _error_code: u64,
) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT\n{:#?}", stack_frame);
}

#[test_case]
fn test_breakpoint_exception() {
    // invoke a breakpoint exception
    x86_64::instructions::interrupts::int3();
}
