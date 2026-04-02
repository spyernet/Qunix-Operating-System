use core::arch::global_asm;

#[repr(C)]
pub struct InterruptFrame {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9: u64,  pub r8: u64,
    pub rbp: u64, pub rdi: u64, pub rsi: u64, pub rdx: u64,
    pub rcx: u64, pub rbx: u64, pub rax: u64,
    pub int_no: u64,
    pub err_code: u64,
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

macro_rules! isr_no_err {
    ($name:ident, $num:expr) => {
        global_asm!(concat!(
            ".global ", stringify!($name), "\n",
            stringify!($name), ":\n",
            "push 0\n",
            "push ", $num, "\n",
            "jmp isr_common_stub\n"
        ));
    };
}

macro_rules! isr_err {
    ($name:ident, $num:expr) => {
        global_asm!(concat!(
            ".global ", stringify!($name), "\n",
            stringify!($name), ":\n",
            "push ", $num, "\n",
            "jmp isr_common_stub\n"
        ));
    };
}

macro_rules! irq_stub {
    ($name:ident, $num:expr) => {
        global_asm!(concat!(
            ".global ", stringify!($name), "\n",
            stringify!($name), ":\n",
            "push 0\n",
            "push ", $num, "\n",
            "jmp irq_common_stub\n"
        ));
    };
}

isr_no_err!(isr0,  0);  isr_no_err!(isr1,  1);  isr_no_err!(isr2,  2);
isr_no_err!(isr3,  3);  isr_no_err!(isr4,  4);  isr_no_err!(isr5,  5);
isr_no_err!(isr6,  6);  isr_no_err!(isr7,  7);  isr_err!(isr8,     8);
isr_no_err!(isr9,  9);  isr_err!(isr10,   10);  isr_err!(isr11,   11);
isr_err!(isr12,   12);  isr_err!(isr13,   13);  isr_err!(isr14,   14);
isr_no_err!(isr15, 15); isr_no_err!(isr16, 16); isr_err!(isr17,   17);
isr_no_err!(isr18, 18); isr_no_err!(isr19, 19); isr_no_err!(isr20, 20);
isr_no_err!(isr21, 21); isr_no_err!(isr22, 22); isr_no_err!(isr23, 23);
isr_no_err!(isr24, 24); isr_no_err!(isr25, 25); isr_no_err!(isr26, 26);
isr_no_err!(isr27, 27); isr_no_err!(isr28, 28); isr_no_err!(isr29, 29);
isr_err!(isr30,   30);  isr_no_err!(isr31, 31);

irq_stub!(irq0,  32);  irq_stub!(irq1,  33);  irq_stub!(irq2,  34);
irq_stub!(irq3,  35);  irq_stub!(irq4,  36);  irq_stub!(irq5,  37);
irq_stub!(irq6,  38);  irq_stub!(irq7,  39);  irq_stub!(irq8,  40);
irq_stub!(irq9,  41);  irq_stub!(irq10, 42);  irq_stub!(irq11, 43);
irq_stub!(irq12, 44);  irq_stub!(irq13, 45);  irq_stub!(irq14, 46);
irq_stub!(irq15, 47);

global_asm!(r#"
.global isr_syscall
isr_syscall:
    push 0
    push 0x80
    jmp isr_common_stub

.global isr_common_stub
isr_common_stub:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rdi, rsp
    sub rsp, 8
    call isr_dispatch
    add rsp, 8

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    iretq

.global irq_common_stub
irq_common_stub:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rdi, rsp
    sub rsp, 8
    call irq_dispatch
    add rsp, 8

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    iretq
"#);

#[no_mangle]
pub extern "C" fn isr_dispatch(frame: &mut InterruptFrame) {
    match frame.int_no {
        0x0e => handle_page_fault(frame),
        0x0d => handle_gpf(frame),
        0x08 => handle_double_fault(frame),
        0x06 => handle_invalid_opcode(frame),
        0x80 => { let sf = unsafe { &mut *(frame as *mut _ as *mut crate::arch::x86_64::syscall_entry::SyscallFrame) }; crate::syscall::dispatch(0x80, sf); },
        n => {
            crate::klog!("Unhandled exception #{} err={:#x} rip={:#x}", n, frame.err_code, frame.rip);
        }
    }
}

#[no_mangle]
pub extern "C" fn irq_dispatch(frame: &mut InterruptFrame) {
    let irq = frame.int_no - 32;
    crate::drivers::irq::dispatch(irq as u8, frame);
    pic_eoi(irq as u8);
}

fn handle_page_fault(frame: &InterruptFrame) {
    let cr2: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2); }
    if frame.err_code & 4 != 0 {
        crate::drivers::serial::write_str("[pf] user page fault\n");
    }
    crate::klog!("Page fault at {:#x} accessing {:#x} err={:#x}", frame.rip, cr2, frame.err_code);
    if frame.err_code & 4 != 0 {
        crate::process::kill_current(crate::signal::SIGSEGV);
    } else {
        panic!("Kernel page fault at {:#x} cr2={:#x}", frame.rip, cr2);
    }
}

fn handle_gpf(frame: &InterruptFrame) {
    if frame.cs & 3 == 3 {
        crate::klog!("User GPF at rip={:#x}", frame.rip);
        crate::process::kill_current(crate::signal::SIGSEGV);
    } else {
        panic!("Kernel GPF at rip={:#x} err={:#x}", frame.rip, frame.err_code);
    }
}

fn handle_double_fault(frame: &InterruptFrame) {
    panic!("Double fault at rip={:#x}", frame.rip);
}

fn handle_invalid_opcode(frame: &InterruptFrame) {
    if frame.cs & 3 == 3 {
        crate::process::kill_current(crate::signal::SIGILL);
    } else {
        panic!("Invalid opcode at rip={:#x}", frame.rip);
    }
}

pub fn pic_eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            crate::arch::x86_64::port::outb(0xA0, 0x20);
        }
        crate::arch::x86_64::port::outb(0x20, 0x20);
    }
}

pub fn pic_init() {
    use crate::arch::x86_64::port::outb;
    unsafe {
        outb(0x20, 0x11);
        outb(0xA0, 0x11);
        outb(0x21, 0x20);
        outb(0xA1, 0x28);
        outb(0x21, 0x04);
        outb(0xA1, 0x02);
        outb(0x21, 0x01);
        outb(0xA1, 0x01);
        outb(0x21, 0x00);
        outb(0xA1, 0x00);
    }
}

/// Page fault handler — handles COW faults and reports real faults.
pub fn page_fault_handler(frame: &InterruptFrame) {
    let fault_addr: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) fault_addr); }
    let error = frame.err_code;
    let is_write = error & 2 != 0;
    let is_user  = error & 4 != 0;

    if is_write && is_user {
        // Try COW resolution
        let handled = crate::process::with_current_mut(|p| {
            p.address_space.handle_cow_fault(fault_addr)
        });
        if handled.unwrap_or(false) {
            crate::perf::PERF.inc_page_fault();
            return;
        }
    }

    // Real fault — deliver SIGSEGV or panic if kernel
    if is_user {
        crate::klog!("Page fault: addr={:#x} err={:#x} rip={:#x}", fault_addr, error, frame.rip);
        crate::signal::send_signal(crate::process::current_pid(), crate::signal::SIGSEGV);
    } else {
        panic!("Kernel page fault at {:#x}, rip={:#x}, err={:#x}", fault_addr, frame.rip, error);
    }
}
