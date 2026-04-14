/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use core::mem::size_of;
use crate::arch::x86_64::tss::Tss;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct GdtEntry {
    limit_low:   u16,
    base_low:    u16,
    base_mid:    u8,
    access:      u8,
    granularity: u8,
    base_high:   u8,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TssDescriptor {
    len:         u16,
    base_low:    u16,
    base_mid:    u8,
    flags1:      u8,
    flags2:      u8,
    base_high:   u8,
    base_upper:  u32,
    _reserved:   u32,
}

#[repr(C, packed)]
pub struct Gdt {
    null:        GdtEntry,
    kernel_code: GdtEntry,
    kernel_data: GdtEntry,
    user_code32: GdtEntry,
    user_data:   GdtEntry,
    user_code64: GdtEntry,
    tss:         TssDescriptor,
}

#[repr(C, packed)]
pub struct GdtPointer {
    limit: u16,
    base:  u64,
}

pub const KERNEL_CODE_SEL: u16 = 0x08;
pub const KERNEL_DATA_SEL: u16 = 0x10;
pub const USER_CODE32_SEL: u16 = 0x18;
pub const USER_DATA_SEL:   u16 = 0x20 | 3;
pub const USER_CODE_SEL:   u16 = 0x28 | 3;
pub const TSS_SEL:         u16 = 0x30;

static mut GDT: Gdt = Gdt {
    null:        GdtEntry { limit_low: 0,      base_low: 0, base_mid: 0, access: 0x00, granularity: 0x00, base_high: 0 },
    kernel_code: GdtEntry { limit_low: 0xffff, base_low: 0, base_mid: 0, access: 0x9a, granularity: 0xaf, base_high: 0 },
    kernel_data: GdtEntry { limit_low: 0xffff, base_low: 0, base_mid: 0, access: 0x92, granularity: 0xcf, base_high: 0 },
    user_code32: GdtEntry { limit_low: 0xffff, base_low: 0, base_mid: 0, access: 0xfa, granularity: 0xcf, base_high: 0 },
    user_data:   GdtEntry { limit_low: 0xffff, base_low: 0, base_mid: 0, access: 0xf2, granularity: 0xcf, base_high: 0 },
    user_code64: GdtEntry { limit_low: 0xffff, base_low: 0, base_mid: 0, access: 0xfa, granularity: 0xaf, base_high: 0 },
    tss: TssDescriptor { len: 0, base_low: 0, base_mid: 0, flags1: 0, flags2: 0, base_high: 0, base_upper: 0, _reserved: 0 },
};

static mut TSS_STORAGE: Tss = Tss::new();

pub fn init() {
    unsafe {
        let tss_addr = core::ptr::addr_of!(TSS_STORAGE) as u64;
        let tss_len  = (size_of::<Tss>() - 1) as u16;

        core::ptr::addr_of_mut!(GDT.tss).write(TssDescriptor {
            len:        tss_len,
            base_low:   tss_addr as u16,
            base_mid:   (tss_addr >> 16) as u8,
            flags1:     0x89,
            flags2:     0x00,
            base_high:  (tss_addr >> 24) as u8,
            base_upper: (tss_addr >> 32) as u32,
            _reserved:  0,
        });

        let gdtp = GdtPointer {
            limit: (size_of::<Gdt>() - 1) as u16,
            base:  core::ptr::addr_of!(GDT) as u64,
        };

        core::arch::asm!(
            "lgdt ({gdtp})",
            "push {cs}",
            "lea 1f(%rip), {tmp}",
            "push {tmp}",
            "lretq",
            "1:",
            "mov {kds:x}, {ds:x}",
            "mov {ds:x}, %ds",
            "mov {ds:x}, %es",
            "mov {ds:x}, %ss",
            "xor {ds:e}, {ds:e}",
            "mov {ds:x}, %fs",
            "mov {ds:x}, %gs",
            gdtp  = in(reg) &gdtp,
            cs    = in(reg) KERNEL_CODE_SEL as u64,
            kds   = in(reg) KERNEL_DATA_SEL as u64,
            tmp   = lateout(reg) _,
            ds    = lateout(reg) _,
            options(att_syntax)
        );

        core::arch::asm!("ltr {0:x}", in(reg) TSS_SEL, options(nostack));
    }
}

pub fn set_kernel_stack(rsp: u64) {
    unsafe {
        core::ptr::addr_of_mut!(TSS_STORAGE).as_mut().unwrap().rsp0 = rsp;
    }
}
