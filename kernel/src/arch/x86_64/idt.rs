/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

#![allow(dead_code)]

use core::mem::size_of;
use crate::arch::x86_64::gdt::KERNEL_CODE_SEL;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IdtEntry {
    offset_low: u16,
    selector: u16,
    ist: u8,
    type_attr: u8,
    offset_mid: u16,
    offset_high: u32,
    _reserved: u32,
}

#[repr(C, packed)]
pub struct IdtPointer {
    limit: u16,
    base: u64,
}

impl IdtEntry {
    pub const fn missing() -> Self {
        IdtEntry {
            offset_low: 0,
            selector: 0,
            ist: 0,
            type_attr: 0,
            offset_mid: 0,
            offset_high: 0,
            _reserved: 0,
        }
    }

    pub fn set_handler(&mut self, handler: u64, ist: u8) {
        self.offset_low = handler as u16;
        self.selector = KERNEL_CODE_SEL;
        self.ist = ist & 0x7;
        self.type_attr = 0x8e;
        self.offset_mid = (handler >> 16) as u16;
        self.offset_high = (handler >> 32) as u32;
        self._reserved = 0;
    }

    pub fn set_trap_handler(&mut self, handler: u64, ist: u8) {
        self.set_handler(handler, ist);
        self.type_attr = 0x8f;
    }
}

static mut IDT: [IdtEntry; 256] = [IdtEntry::missing(); 256];

extern "C" {
    fn isr0();  fn isr1();  fn isr2();  fn isr3();
    fn isr4();  fn isr5();  fn isr6();  fn isr7();
    fn isr8();  fn isr9();  fn isr10(); fn isr11();
    fn isr12(); fn isr13(); fn isr14(); fn isr15();
    fn isr16(); fn isr17(); fn isr18(); fn isr19();
    fn isr20(); fn isr21(); fn isr22(); fn isr23();
    fn isr24(); fn isr25(); fn isr26(); fn isr27();
    fn isr28(); fn isr29(); fn isr30(); fn isr31();
    fn irq0();  fn irq1();  fn irq2();  fn irq3();
    fn irq4();  fn irq5();  fn irq6();  fn irq7();
    fn irq8();  fn irq9();  fn irq10(); fn irq11();
    fn irq12(); fn irq13(); fn irq14(); fn irq15();
    fn isr_syscall();
}

pub fn init() {
    unsafe {
        IDT[0].set_handler(isr0 as u64, 0);
        IDT[1].set_handler(isr1 as u64, 0);
        IDT[2].set_handler(isr2 as u64, 0);
        IDT[3].set_handler(isr3 as u64, 0);
        IDT[4].set_handler(isr4 as u64, 0);
        IDT[5].set_handler(isr5 as u64, 0);
        IDT[6].set_handler(isr6 as u64, 0);
        IDT[7].set_handler(isr7 as u64, 0);
        IDT[8].set_handler(isr8 as u64, 1);
        IDT[9].set_handler(isr9 as u64, 0);
        IDT[10].set_handler(isr10 as u64, 0);
        IDT[11].set_handler(isr11 as u64, 0);
        IDT[12].set_handler(isr12 as u64, 0);
        IDT[13].set_handler(isr13 as u64, 0);
        IDT[14].set_handler(isr14 as u64, 0);
        IDT[15].set_handler(isr15 as u64, 0);
        IDT[16].set_handler(isr16 as u64, 0);
        IDT[17].set_handler(isr17 as u64, 0);
        IDT[18].set_handler(isr18 as u64, 0);
        IDT[19].set_handler(isr19 as u64, 0);
        IDT[20].set_handler(isr20 as u64, 0);
        IDT[21].set_handler(isr21 as u64, 0);
        IDT[22].set_handler(isr22 as u64, 0);
        IDT[23].set_handler(isr23 as u64, 0);
        IDT[24].set_handler(isr24 as u64, 0);
        IDT[25].set_handler(isr25 as u64, 0);
        IDT[26].set_handler(isr26 as u64, 0);
        IDT[27].set_handler(isr27 as u64, 0);
        IDT[28].set_handler(isr28 as u64, 0);
        IDT[29].set_handler(isr29 as u64, 0);
        IDT[30].set_handler(isr30 as u64, 0);
        IDT[31].set_handler(isr31 as u64, 0);

        IDT[32].set_handler(irq0 as u64, 0);
        IDT[33].set_handler(irq1 as u64, 0);
        IDT[34].set_handler(irq2 as u64, 0);
        IDT[35].set_handler(irq3 as u64, 0);
        IDT[36].set_handler(irq4 as u64, 0);
        IDT[37].set_handler(irq5 as u64, 0);
        IDT[38].set_handler(irq6 as u64, 0);
        IDT[39].set_handler(irq7 as u64, 0);
        IDT[40].set_handler(irq8 as u64, 0);
        IDT[41].set_handler(irq9 as u64, 0);
        IDT[42].set_handler(irq10 as u64, 0);
        IDT[43].set_handler(irq11 as u64, 0);
        IDT[44].set_handler(irq12 as u64, 0);
        IDT[45].set_handler(irq13 as u64, 0);
        IDT[46].set_handler(irq14 as u64, 0);
        IDT[47].set_handler(irq15 as u64, 0);

        IDT[0x80].set_trap_handler(isr_syscall as u64, 0);
        IDT[0x80].type_attr |= 0x60;

        let idtp = IdtPointer {
            limit: (size_of::<[IdtEntry; 256]>() - 1) as u16,
            base: IDT.as_ptr() as u64,
        };

        core::arch::asm!("lidt [{0}]", in(reg) &idtp as *const IdtPointer);
        core::arch::asm!("sti");
    }
}
