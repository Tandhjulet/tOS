use crate::{
    hlt_loop, println, serial_println,
    sys::{
        acpi::{
            ACPI,
            sdt::madt::{Madt, MadtEntry},
        },
        gdt,
        interrupts::apic::{ApicInfo, CpuInfo, IntSourceOverride, IoApicInfo, LapicNmi},
    },
};
use alloc::{boxed::Box, string::String, vec::Vec};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use seq_macro::seq;
use spin::Mutex;
use x86_64::{
    instructions::interrupts::without_interrupts,
    structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode},
};

pub mod apic;

pub const MIN_INTERRUPT: usize = 32;
pub const PIC_1_OFFSET: u8 = MIN_INTERRUPT as u8;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static INTERRUPT_CONTROLLER: InterruptController =
    InterruptController::new(InterruptControllerType::Pic(unsafe {
        ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET)
    }));

pub struct InterruptController(Mutex<InterruptControllerType>);

impl InterruptController {
    pub const fn new(int_type: InterruptControllerType) -> Self {
        Self(Mutex::new(int_type))
    }

    pub fn get(&self) -> spin::MutexGuard<'_, InterruptControllerType> {
        self.0.lock()
    }

    pub fn init(&self) {
        self.get().init();
    }

    pub fn eoi(&self, id: u8) {
        self.get().eoi(id);
    }

    pub fn switch_controller(&self, new_int_type: InterruptControllerType) {
        let mut guard = self.get();
        guard.disable();
        *guard = new_int_type;
        guard.init();
    }

    pub unsafe fn unmask_irq(&self, irq: u8) -> Result<(), String> {
        unsafe { self.get().unmask_irq(irq) }
    }
}

pub enum InterruptMode {
    MsiX,
    Msi,
    Isa,
}

pub enum InterruptControllerType {
    Apic(ApicInfo),
    Pic(ChainedPics),
}

impl InterruptControllerType {
    pub unsafe fn unmask_irq(&mut self, irq: u8) -> Result<(), String> {
        match self {
            InterruptControllerType::Apic(apic) => apic.unmask_irq(irq),
            InterruptControllerType::Pic(pics) => unsafe {
                let [master, slave] = pics.read_masks();
                if irq < 8 {
                    pics.write_masks(master & !(1u8 << irq), slave);
                } else {
                    pics.write_masks(master, slave & !(1u8 << (irq - 8)));
                }

                Ok(())
            },
        }
    }

    pub fn disable(&mut self) {
        match self {
            InterruptControllerType::Apic(_) => todo!(),
            InterruptControllerType::Pic(chained_pics) => {
                unsafe { chained_pics.disable() };
            }
        }
    }

    pub fn eoi(&mut self, id: u8) {
        match self {
            InterruptControllerType::Apic(apic) => {
                apic.lapic.eoi();
            }
            InterruptControllerType::Pic(pics) => unsafe {
                pics.notify_end_of_interrupt(id);
            },
        }
    }

    pub fn init(&mut self) {
        match self {
            InterruptControllerType::Apic(info) => {
                unsafe { info.lapic.enable() };

                let iso = info.iso.clone();
                for ioapic in &mut info.ioapics {
                    unsafe { ioapic.init(&info.lapic, &iso) };
                }
            }
            InterruptControllerType::Pic(pics) => unsafe {
                pics.initialize();

                let [master, slave] = pics.read_masks();
                pics.write_masks(master & !(1 << 2), slave);
            },
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard,
}

seq!(N in 32..=255 {
    extern "x86-interrupt" fn irq_handler_~N(_stack_frame: InterruptStackFrame) {
        let handlers = HANDLERS.lock();
        let handler = handlers.get(N - 32).expect("Length of HANDLERS should be 256 - 32");

        match handler {
            Some(f) => match f() {
                IrqResult::EoiNeeded => INTERRUPT_CONTROLLER.eoi(N as u8),
                IrqResult::EoiSent => {},
            },
            None => INTERRUPT_CONTROLLER.eoi(N as u8),
        }
    }
});

pub enum IrqResult {
    EoiSent,
    EoiNeeded,
}

pub static HANDLERS: Mutex<Vec<Option<Box<dyn Fn() -> IrqResult + Send>>>> = Mutex::new(Vec::new());

pub fn init_handlers() {
    let mut h = HANDLERS.lock();
    let curr_len = h.len();
    h.reserve_exact(256 - 32 - curr_len);
    h.extend((0..256 - 32).map(|_| None));
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);

        seq!(N in 32..=255 {
            idt[N].set_handler_fn(irq_handler_~N);
        });

        idt
    };
}

pub fn load_idt() {
    IDT.load();
}

pub fn init() {
    init_handlers();

    load_idt();
    INTERRUPT_CONTROLLER.init();

    register_handler(
        InterruptIndex::Timer as u8,
        Box::new(timer_interrupt_handler),
    );
    register_handler(
        InterruptIndex::Keyboard as u8,
        Box::new(keyboard_interrupt_handler),
    );
}

pub fn enable_isa_irq() -> Result<(), String> {
    enable_interrupt(InterruptIndex::Timer as u8)?;
    enable_interrupt(InterruptIndex::Keyboard as u8)?;
    Ok(())
}

pub fn allocate_interrupt(handler: Box<dyn Fn() -> IrqResult + Send>) -> Option<u8> {
    without_interrupts(|| {
        let mut handlers = HANDLERS.lock();
        let idx = handlers.iter().position(|h| h.is_none())?;
        handlers[idx] = Some(handler);
        Some(idx as u8 + MIN_INTERRUPT as u8)
    })
}

pub fn register_handler(vector: u8, handler: Box<dyn Fn() -> IrqResult + Send>) {
    without_interrupts(|| HANDLERS.lock()[vector as usize - MIN_INTERRUPT] = Some(handler))
}

/**
 * Maps interrupt to IRQ line and enables that line
 */
pub fn enable_interrupt(vector: u8) -> Result<(), String> {
    without_interrupts(|| unsafe { INTERRUPT_CONTROLLER.unmask_irq(vector - MIN_INTERRUPT as u8) })
}

pub fn try_init_apic() -> Result<(), &'static str> {
    {
        if matches!(
            *INTERRUPT_CONTROLLER.get(),
            InterruptControllerType::Apic(..)
        ) {
            return Err("APIC already initialized");
        }
    }

    let acpi = ACPI.get().ok_or("Failed to find ACPI")?;
    let madt_table = acpi
        .tables()
        .find_table::<Madt>()
        .ok_or("Failed to find MADT table in ACPI")?;

    let madt = madt_table.get();

    let mut apic_info = ApicInfo::new(madt.lapic_addr as u64);

    let entries = madt.entries();
    for entry in entries {
        match entry {
            MadtEntry::LocalApic(cpu) => {
                apic_info.cpus.push(CpuInfo {
                    processor_id: cpu.acpi_processor_id,
                    apic_id: cpu.apic_id,
                    flags: cpu.flags,
                    nmis: Vec::new(),
                });
            }
            MadtEntry::IoApic(io) => {
                apic_info.ioapics.push(IoApicInfo::new(
                    io.io_apic_id,
                    io.io_apic_addr as u64,
                    io.gsi_base,
                ));
            }
            MadtEntry::InterruptSourceOverride(iso) => {
                apic_info.iso.push(IntSourceOverride {
                    bus: iso.bus_src,
                    bus_irq: iso.bus_irq,
                    gsi: iso.gsi,
                    flags: iso.flags,
                });
            }
            MadtEntry::LocalApicNmi(nmi) => {
                let lapic_nmi = LapicNmi {
                    flags: nmi.flags,
                    lint: nmi.lint,
                };

                if nmi.acpi_processor_id == 0xFF {
                    apic_info.cpus.iter_mut().for_each(|cpu| {
                        cpu.nmis.push(lapic_nmi.clone());
                    });

                    continue;
                }

                let Some(cpu) = apic_info
                    .cpus
                    .iter_mut()
                    .find(|cpu| cpu.processor_id == nmi.acpi_processor_id)
                else {
                    println!(
                        "Received Local APIC NMI for CPU with id {} but could not find it.",
                        { nmi.acpi_processor_id }
                    );
                    continue;
                };

                cpu.nmis.push(lapic_nmi);
            }
        }
    }

    INTERRUPT_CONTROLLER.switch_controller(InterruptControllerType::Apic(apic_info));
    Ok(())
}

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

fn timer_interrupt_handler() -> IrqResult {
    IrqResult::EoiNeeded
}

fn keyboard_interrupt_handler() -> IrqResult {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::sys::task::keyboard::add_scancode(scancode);

    IrqResult::EoiNeeded
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    println!("EXCEPTION: BREAKPOINT\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    serial_println!("DOUBLE FAULT");
    serial_println!("IP: {:#x}", stack_frame.instruction_pointer.as_u64());
    serial_println!("SP: {:#x}", stack_frame.stack_pointer.as_u64());
    serial_println!("code seg: {:#x}", stack_frame.code_segment.index());

    panic!(
        "EXCEPTION: DOUBLE FAULT\n{:#?} (Code: {})",
        stack_frame, error_code
    );
}

#[test_case]
fn test_breakpoint_exception() {
    // invoke a breakpoint exception
    x86_64::instructions::interrupts::int3();
}
