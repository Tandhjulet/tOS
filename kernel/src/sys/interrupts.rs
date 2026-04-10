use crate::{
    hlt_loop, println,
    sys::{
        acpi::{
            ACPI,
            sdt::madt::{Madt, MadtEntry},
        },
        gdt,
    },
};
use alloc::vec::Vec;
use pic8259::ChainedPics;
use spin::{Mutex, MutexGuard};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

pub const MIN_INTERRUPT: usize = 32;
pub const PIC_1_OFFSET: u8 = MIN_INTERRUPT as u8;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static INTERRUPT_CONTROLLER: InterruptController =
    InterruptController::new(InterruptType::Pic(unsafe {
        ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET)
    }));

pub struct InterruptController {
    controller: Mutex<InterruptType>,
}

impl InterruptController {
    pub const fn new(int_type: InterruptType) -> Self {
        Self {
            controller: Mutex::new(int_type),
        }
    }

    pub fn init(&self) {
        let mut lock = self.lock();
        lock.init();
    }

    pub fn lock(&self) -> MutexGuard<'_, InterruptType> {
        self.controller.lock()
    }

    pub fn eoi(&self, id: u8) {
        self.lock().eoi(id);
    }

    pub fn switch_controller(&self, new_int_type: InterruptType) {
        let mut curr_ctlr = self.lock();
        curr_ctlr.disable();
        *curr_ctlr = new_int_type;
    }

    pub fn unmask_irq(&self, irq: u8) {
        self.lock().unmask_irq(irq);
    }
}

pub enum InterruptType {
    Apic(ApicInfo),
    Pic(ChainedPics),
}

impl InterruptType {
    pub fn unmask_irq(&mut self, irq: u8) {
        match self {
            InterruptType::Apic(_) => todo!(),
            InterruptType::Pic(pics) => unsafe {
                let [master, slave] = pics.read_masks();
                if irq < 8 {
                    pics.write_masks(master & !(1u8 << irq), slave);
                } else {
                    pics.write_masks(master, slave & !(1u8 << (irq - 8)));
                }
            },
        }
    }

    pub fn disable(&mut self) {
        match self {
            InterruptType::Apic(_) => todo!(),
            InterruptType::Pic(chained_pics) => {
                unsafe { (*chained_pics).disable() };
            }
        }
    }

    pub fn eoi(&mut self, id: u8) {
        match self {
            InterruptType::Apic(_) => todo!(),
            InterruptType::Pic(chained_pics) => unsafe { chained_pics.notify_end_of_interrupt(id) },
        }
    }

    pub fn init(&mut self) {
        match self {
            InterruptType::Apic(_) => todo!(),
            InterruptType::Pic(pics) => unsafe {
                pics.initialize();

                let [master, slave] = pics.read_masks();
                pics.write_masks(master & !(1u8 << 2), slave);
            },
        }
    }
}

pub struct ApicInfo {
    pub lapic_addr: u32,
    pub ioapics: Vec<IoApicInfo>,
    pub iso: Vec<IntSourceOverride>,
    pub cpus: Vec<CpuInfo>,
}

impl ApicInfo {
    pub fn new(lapic_addr: u32) -> Self {
        Self {
            lapic_addr,
            ioapics: Vec::new(),
            iso: Vec::new(),
            cpus: Vec::new(),
        }
    }
}

pub struct IoApicInfo {
    pub id: u8,
    pub addr: u32,
    pub gsi_base: u32,
    pub nmis: Vec<IoApicNmi>,
}

pub struct IntSourceOverride {
    pub bus: u8,
    pub bus_irq: u8,
    pub gsi: u32,
    pub flags: u16,
}

pub struct CpuInfo {
    pub processor_id: u8,
    pub apic_id: u8,
    pub flags: u32,
    pub nmis: Vec<LapicNmi>,
}

#[derive(Clone, Copy)]
pub struct LapicNmi {
    pub flags: u16,
    pub lint: u8,
}

#[derive(Clone, Copy)]
pub struct IoApicNmi {
    pub nmi_src: u8,
    _reserved: u8,
    pub flags: u16,
    pub gsi: u32,
}

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

pub fn init_idt() {
    {
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
    }

    load_idt();
}

pub fn load_idt() {
    let idt = IDT.lock();
    let idt_static: &'static InterruptDescriptorTable = unsafe { &*(&*idt as *const _) };
    idt_static.load();
}

pub fn try_init_apic() -> Result<(), &'static str> {
    {
        let curr_int_ctrlr = INTERRUPT_CONTROLLER.lock();
        if matches!(*curr_int_ctrlr, InterruptType::Apic(..)) {
            return Err("APIC already initialized");
        }
    }

    let acpi = ACPI.get().ok_or("Failed to find ACPI")?;
    let madt_table = acpi
        .tables()
        .find_table::<Madt>()
        .ok_or("Failed to find MADT table in ACPI")?;

    let madt = madt_table.get();

    let mut apic_info = ApicInfo::new(madt.lapic_addr);

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
                apic_info.ioapics.push(IoApicInfo {
                    id: io.io_apic_id,
                    addr: io.io_apic_addr,
                    gsi_base: io.gsi_base,
                    nmis: Vec::new(),
                });
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

    INTERRUPT_CONTROLLER.switch_controller(InterruptType::Apic(apic_info));
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

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    INTERRUPT_CONTROLLER.eoi(InterruptIndex::Timer.as_u8());
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;

    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::sys::task::keyboard::add_scancode(scancode);

    INTERRUPT_CONTROLLER.eoi(InterruptIndex::Timer.as_u8());
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
