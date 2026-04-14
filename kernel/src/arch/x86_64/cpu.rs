/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use crate::arch::x86_64::msr;
use crate::arch::x86_64::gdt::KERNEL_CODE_SEL;

pub fn init() {
    unsafe {
        enable_sse();
        enable_syscall_msr();
    }
    crate::arch::x86_64::interrupts::pic_init();
    crate::klog!("CPU initialized (x86_64)");
}

unsafe fn enable_sse() {
    let mut cr0: u64;
    core::arch::asm!("mov {}, cr0", out(reg) cr0);
    cr0 &= !(1u64 << 2); // clear EM
    cr0 |= 1u64 << 1;    // set MP
    core::arch::asm!("mov cr0, {}", in(reg) cr0);

    let mut cr4: u64;
    core::arch::asm!("mov {}, cr4", out(reg) cr4);
    cr4 |= (1u64 << 9) | (1u64 << 10); // OSFXSR + OSXMMEXCPT
    core::arch::asm!("mov cr4, {}", in(reg) cr4);
}

unsafe fn enable_syscall_msr() {
    // set SCE bit in EFER
    let efer = msr::read(msr::IA32_EFER);
    msr::write(msr::IA32_EFER, efer | 1);

    // STAR: ring 0 CS = KERNEL_CODE_SEL, ring 3 CS = USER_CODE32_SEL (syscall uses +16 for ret)
    // High 32: sysret cs/ss selectors  (bits 63:48 = user cs, bits 47:32 = kernel cs)
    let star_hi: u64 = ((crate::arch::x86_64::gdt::USER_DATA_SEL as u64 - 8) << 16)
        | (KERNEL_CODE_SEL as u64);
    msr::write(msr::IA32_STAR, star_hi << 32);

    msr::write(msr::IA32_LSTAR, crate::arch::x86_64::syscall_entry::syscall_entry as u64);

    // mask IF on syscall entry
    msr::write(msr::IA32_FMASK, 0x200);
}

pub fn halt() -> ! {
    unsafe {
        loop { core::arch::asm!("hlt"); }
    }
}

pub fn enable_interrupts() {
    unsafe { core::arch::asm!("sti") }
}

pub fn disable_interrupts() {
    unsafe { core::arch::asm!("cli") }
}

pub fn interrupts_enabled() -> bool {
    let flags: u64;
    unsafe { core::arch::asm!("pushfq; pop {}", out(reg) flags) }
    flags & (1 << 9) != 0
}

pub struct IrqGuard(bool);

impl IrqGuard {
    pub fn new() -> Self {
        let was = interrupts_enabled();
        disable_interrupts();
        IrqGuard(was)
    }
}

impl Drop for IrqGuard {
    fn drop(&mut self) {
        if self.0 { enable_interrupts(); }
    }
}

pub fn cpuid(leaf: u32) -> (u32, u32, u32, u32) {
    let (eax, ecx, edx): (u32, u32, u32);
    let ebx: u32;
    unsafe {
        core::arch::asm!(
            "push rbx",
            "cpuid",
            "mov {ebx_out:e}, ebx",
            "pop rbx",
            inout("eax") leaf => eax,
            ebx_out = out(reg) ebx,
            inout("ecx") 0u32 => ecx,
            lateout("edx") edx,
        );
    }
    (eax, ebx, ecx, edx)
}

pub unsafe fn halt_forever() -> ! { loop { core::arch::asm!("cli; hlt"); } }

/// Initialize CPU-specific features on an Application Processor.
/// Called from ap_entry_64 for each non-BSP core.
pub fn init_ap() {
    unsafe {
        enable_sse();
        enable_syscall_msr();
    }
}
