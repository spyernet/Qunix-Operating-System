/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! SMP — per-CPU data, APIC, IPI, TLB shootdown, AP startup.
//!
//! GS base points to PerCpuData. The layout of the FIRST 24 bytes is
//! ABI-fixed because the syscall entry ASM hard-codes offsets:
//!
//!   gs:[0]  = kernel_rsp  (u64) — loaded by syscall entry
//!   gs:[8]  = cpu_id      (u32) — read by current_cpu_id()
//!   gs:[12] = apic_id     (u32)
//!   gs:[16] = user_rsp    (u64) — scratch for saving user RSP on syscall
//!
//! DO NOT reorder or insert fields before offset 24 without updating
//! syscall_entry.rs assembly and current_cpu_id().

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const MAX_CPUS: usize = 64;

// ── Per-CPU data — layout is ABI-fixed for syscall_entry.rs ──────────────

#[repr(C)]
pub struct PerCpuData {
    // ── ABI-fixed block (offsets 0..24, used by syscall_entry.rs asm) ──
    pub kernel_rsp:  u64,   // @0  : kernel stack RSP, loaded on syscall entry
    pub cpu_id:      u32,   // @8  : logical CPU ID
    pub apic_id:     u32,   // @12 : APIC hardware ID
    pub user_rsp:    u64,   // @16 : scratch — user RSP saved here on syscall

    // ── General per-CPU state ───────────────────────────────────────────
    pub online:      bool,
    pub current_pid: u32,
    pub idle_pid:    u32,
    pub tsc_freq:    u64,   // calibrated TSC frequency Hz

    // ── TLB shootdown state ─────────────────────────────────────────────
    pub tlb_pending: AtomicBool,
    pub tlb_vaddr:   AtomicU64,
}

impl PerCpuData {
    const fn zeroed() -> Self {
        PerCpuData {
            kernel_rsp: 0, cpu_id: 0, apic_id: 0, user_rsp: 0,
            online: false, current_pid: 0, idle_pid: 0, tsc_freq: 0,
            tlb_pending: AtomicBool::new(false),
            tlb_vaddr:   AtomicU64::new(0),
        }
    }
}

// Per-CPU data array (indexed by logical CPU ID)
static mut CPU_DATA: [PerCpuData; MAX_CPUS] = {
    [const { PerCpuData::zeroed() }; MAX_CPUS]
};

pub static CPU_COUNT:      AtomicU32 = AtomicU32::new(1);
pub static CPU_ONLINE_MASK: AtomicU64 = AtomicU64::new(1);

static AP_STARTED: AtomicU32 = AtomicU32::new(0);
static BSP_READY:  AtomicBool = AtomicBool::new(false);

// ── APIC MMIO ─────────────────────────────────────────────────────────────

const LOCAL_APIC_BASE_MSR: u32 = 0x1B;
const LOCAL_APIC_ENABLE:   u64 = 1 << 11;

const APIC_EOI:       usize = 0x0B0;
const APIC_SPURIOUS:  usize = 0x0F0;
const APIC_ICR_LO:    usize = 0x300;
const APIC_ICR_HI:    usize = 0x310;
const APIC_LVT_TIMER: usize = 0x320;
const APIC_LVT_LINT0: usize = 0x350;
const APIC_LVT_LINT1: usize = 0x360;
const APIC_LVT_ERROR: usize = 0x370;
const APIC_TIMER_ICR: usize = 0x380;
const APIC_TIMER_DCR: usize = 0x3E0;
const APIC_ID:        usize = 0x020;

const ICR_INIT:       u32 = 5 << 8;
const ICR_STARTUP:    u32 = 6 << 8;
const ICR_ASSERT:     u32 = 1 << 14;
const ICR_LEVEL:      u32 = 1 << 15;
const ICR_DEASSERT:   u32 = 0 << 14;
const ICR_NO_SHORT:   u32 = 0 << 18;
const ICR_ALL_EXCL:   u32 = 3 << 18;
const ICR_FIXED:      u32 = 0 << 8;
const ICR_PENDING:    u32 = 1 << 12;

pub const IPI_RESCHEDULE:    u8 = 0xF0;
pub const IPI_TLB_SHOOTDOWN: u8 = 0xF1;
pub const IPI_STOP:          u8 = 0xF2;

pub fn apic_base() -> u64 {
    unsafe { crate::arch::x86_64::msr::read(LOCAL_APIC_BASE_MSR) & !0xFFF }
}

pub fn apic_read(reg: usize) -> u32 {
    let v = apic_base() + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
    unsafe { core::ptr::read_volatile((v + reg as u64) as *const u32) }
}

pub fn apic_write(reg: usize, val: u32) {
    let v = apic_base() + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
    unsafe { core::ptr::write_volatile((v + reg as u64) as *mut u32, val); }
}

pub fn apic_id() -> u32 { apic_read(APIC_ID) >> 24 }
pub fn apic_eoi()       { apic_write(APIC_EOI, 0); }

// ── BSP init ─────────────────────────────────────────────────────────────

pub fn bsp_init() {
    // Enable APIC
    let base = unsafe { crate::arch::x86_64::msr::read(LOCAL_APIC_BASE_MSR) };
    unsafe { crate::arch::x86_64::msr::write(LOCAL_APIC_BASE_MSR, base | LOCAL_APIC_ENABLE); }

    apic_write(APIC_SPURIOUS, 0x1FF);      // enable, spurious vector = 0xFF
    apic_write(APIC_LVT_LINT0, 0x10000);   // mask LINT0
    apic_write(APIC_LVT_LINT1, 0x10000);   // mask LINT1
    apic_write(APIC_LVT_ERROR, 0x10000);   // mask ERROR

    // Keep the LAPIC enabled for IPIs/SMP, but leave its timer masked.
    // Early boot timekeeping already uses the PIT on IRQ0; enabling the LAPIC
    // timer here would deliver vector 0x40 before we install a handler for it.
    apic_write(APIC_TIMER_DCR, 0x3);       // divide by 16
    apic_write(APIC_LVT_TIMER, 0x10040);   // masked, vector 0x40 reserved
    apic_write(APIC_TIMER_ICR, 0x100000);

    unsafe {
        CPU_DATA[0].cpu_id  = 0;
        CPU_DATA[0].apic_id = apic_id();
        CPU_DATA[0].online  = true;

        // Point GS at our per-CPU data
        let gs_val = &CPU_DATA[0] as *const PerCpuData as u64;
        crate::arch::x86_64::msr::write(crate::arch::x86_64::msr::IA32_GSBASE, gs_val);
        crate::arch::x86_64::msr::write(crate::arch::x86_64::msr::IA32_KERNEL_GSBASE, gs_val);
    }

    crate::klog!("APIC: BSP init, ID={}", apic_id());
}

/// Update the kernel RSP in the per-CPU data.
/// Must be called whenever the current process's kernel stack changes.
pub fn set_kernel_stack(rsp: u64) {
    unsafe {
        let cpu = current_cpu_id() as usize;
        if cpu < MAX_CPUS { CPU_DATA[cpu].kernel_rsp = rsp; }
    }
}

pub fn get_kernel_stack() -> u64 {
    unsafe {
        let cpu = current_cpu_id() as usize;
        if cpu < MAX_CPUS { CPU_DATA[cpu].kernel_rsp } else { 0 }
    }
}

/// Read the user RSP that syscall_entry saved into gs:[16].
/// Valid only while inside a syscall handler (between syscall_entry and sysretq).
pub fn get_user_rsp() -> u64 {
    unsafe {
        let cpu = current_cpu_id() as usize;
        if cpu < MAX_CPUS { CPU_DATA[cpu].user_rsp } else { 0 }
    }
}

/// Overwrite the user RSP in gs:[16].
/// syscall_exit will load this into rsp before sysretq, so the user
/// process resumes on the new stack (used for signal frame delivery).
pub fn set_user_rsp(rsp: u64) {
    unsafe {
        let cpu = current_cpu_id() as usize;
        if cpu < MAX_CPUS { CPU_DATA[cpu].user_rsp = rsp; }
    }
}

pub fn set_current_pid(pid: u32) {
    unsafe {
        let cpu = current_cpu_id() as usize;
        if cpu < MAX_CPUS { CPU_DATA[cpu].current_pid = pid; }
    }
}

pub fn get_current_pid() -> u32 {
    unsafe {
        let cpu = current_cpu_id() as usize;
        if cpu < MAX_CPUS { CPU_DATA[cpu].current_pid } else { 0 }
    }
}

// ── AP startup ────────────────────────────────────────────────────────────

pub fn start_aps(apic_ids: &[u32]) {
    if apic_ids.is_empty() { return; }
    install_trampoline();
    let bsp = apic_id();
    let mut started = 0u32;
    for (li, &ai) in apic_ids.iter().enumerate() {
        if ai == bsp { continue; }
        let cpu_id = li as u32 + 1;
        if cpu_id >= MAX_CPUS as u32 { break; }
        unsafe {
            CPU_DATA[cpu_id as usize].cpu_id  = cpu_id;
            CPU_DATA[cpu_id as usize].apic_id = ai;
        }
        send_ipi_to(ai, ICR_INIT | ICR_ASSERT | ICR_LEVEL | ICR_NO_SHORT, 0);
        delay_us(10_000);
        send_ipi_to(ai, ICR_INIT | ICR_DEASSERT | ICR_LEVEL | ICR_NO_SHORT, 0);
        delay_us(10_000);
        for _ in 0..2 {
            send_ipi_to(ai, ICR_STARTUP | ICR_NO_SHORT, 0x08);
            delay_us(200);
        }
        let deadline = crate::time::ticks() + 100;
        while crate::time::ticks() < deadline {
            if AP_STARTED.load(Ordering::Acquire) > started { break; }
            core::hint::spin_loop();
        }
        started = AP_STARTED.load(Ordering::Acquire);
    }
    BSP_READY.store(true, Ordering::Release);
}

fn install_trampoline() {
    use crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
    // Write CR3 value at a well-known physical location for the AP trampoline
    let cr3 = crate::arch::x86_64::paging::get_cr3() as u32;
    unsafe {
        let tramp_cr3 = (0x7000u64 + KERNEL_VIRT_OFFSET) as *mut u32;
        *tramp_cr3 = cr3;
    }
}

/// 64-bit AP entry — called from trampoline.
#[no_mangle]
pub unsafe extern "C" fn ap_entry_64() -> ! {
    crate::arch::x86_64::gdt::init();
    crate::arch::x86_64::idt::init();
    bsp_init();

    let my_apic = apic_id();
    let mut my_cpu = 0u32;
    for i in 1..MAX_CPUS {
        if CPU_DATA[i].apic_id == my_apic { my_cpu = i as u32; break; }
    }

    let gs_val = &CPU_DATA[my_cpu as usize] as *const PerCpuData as u64;
    crate::arch::x86_64::msr::write(crate::arch::x86_64::msr::IA32_GSBASE,  gs_val);
    crate::arch::x86_64::msr::write(crate::arch::x86_64::msr::IA32_KERNEL_GSBASE, gs_val);

    CPU_DATA[my_cpu as usize].online = true;
    crate::arch::x86_64::cpu::init_ap();

    CPU_COUNT.fetch_add(1, Ordering::Release);
    CPU_ONLINE_MASK.fetch_or(1 << my_cpu, Ordering::Release);
    AP_STARTED.fetch_add(1, Ordering::Release);

    while !BSP_READY.load(Ordering::Acquire) { core::hint::spin_loop(); }
    ap_idle()
}

fn ap_idle() -> ! {
    loop {
        crate::arch::x86_64::cpu::enable_interrupts();
        unsafe { core::arch::asm!("hlt"); }
        crate::sched::schedule();
    }
}

// ── IPI ───────────────────────────────────────────────────────────────────

pub fn send_ipi_to(dest_apic: u32, flags: u32, vector: u8) {
    apic_write(APIC_ICR_HI, dest_apic << 24);
    apic_write(APIC_ICR_LO, flags | vector as u32);
    while apic_read(APIC_ICR_LO) & ICR_PENDING != 0 { core::hint::spin_loop(); }
}

pub fn send_ipi_all(vector: u8) {
    apic_write(APIC_ICR_HI, 0);
    apic_write(APIC_ICR_LO, ICR_FIXED | ICR_ALL_EXCL | vector as u32);
    while apic_read(APIC_ICR_LO) & ICR_PENDING != 0 { core::hint::spin_loop(); }
}

// ── TLB shootdown ─────────────────────────────────────────────────────────

pub fn tlb_shootdown(virt: u64) {
    unsafe { crate::arch::x86_64::paging::invalidate_tlb(virt); }
    let n = CPU_COUNT.load(Ordering::Relaxed);
    if n <= 1 { return; }
    let me = current_cpu_id();
    for cpu in 0..n as usize {
        if cpu as u32 == me { continue; }
        unsafe {
            CPU_DATA[cpu].tlb_vaddr.store(virt, Ordering::Release);
            CPU_DATA[cpu].tlb_pending.store(true, Ordering::Release);
        }
    }
    send_ipi_all(IPI_TLB_SHOOTDOWN);
    for cpu in 0..n as usize {
        if cpu as u32 == me { continue; }
        while unsafe { CPU_DATA[cpu].tlb_pending.load(Ordering::Acquire) } {
            core::hint::spin_loop();
        }
    }
}

pub fn handle_tlb_shootdown() {
    let cpu = current_cpu_id() as usize;
    unsafe {
        let v = CPU_DATA[cpu].tlb_vaddr.load(Ordering::Acquire);
        crate::arch::x86_64::paging::invalidate_tlb(v);
        CPU_DATA[cpu].tlb_pending.store(false, Ordering::Release);
    }
    apic_eoi();
}

pub fn handle_reschedule_ipi() {
    crate::sched::schedule();
    apic_eoi();
}

// ── CPU helpers ───────────────────────────────────────────────────────────

/// Read logical CPU ID from gs:[8] (the cpu_id field).
#[inline]
pub fn current_cpu_id() -> u32 {
    let id: u32;
    // cpu_id is at offset 8 in PerCpuData
    unsafe { core::arch::asm!("mov {:e}, gs:[8]", out(reg) id); }
    id
}

pub fn cpu_count() -> u32 { CPU_COUNT.load(Ordering::Relaxed) }

fn delay_us(us: u64) {
    let start = crate::time::ticks();
    let wait  = us / 1000 + 1;
    while crate::time::ticks() - start < wait { core::hint::spin_loop(); }
}

pub fn get_current_pid_for_cpu(_cpu: usize) -> u32 { crate::process::current_pid() }
