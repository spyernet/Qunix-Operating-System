/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! VGA text mode console (80x25, color text).
//!
//! The VGA buffer at physical 0xB8000 is accessible via phys_to_virt() once the
//! bootloader's direct physical map (KERNEL_VIRT_OFFSET) is active.
//! That mapping is established by the bootloader before calling kernel_main,
//! so this driver is safe to use from the first line of kernel_main.
//!
//! Serial output is always available as a fallback (debug::KernelLogger sends to both).

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use crate::arch::x86_64::port::outb;
use crate::arch::x86_64::paging::phys_to_virt;

const VGA_PHYS:  u64   = 0xB8000;
const WIDTH:  usize = 80;
const HEIGHT: usize = 25;

static COL:   AtomicUsize = AtomicUsize::new(0);
static ROW:   AtomicUsize = AtomicUsize::new(0);
static COLOR: AtomicUsize = AtomicUsize::new(0x0F); // white on black
static READY: AtomicBool  = AtomicBool::new(false);

pub fn init() {
    // The bootloader maps phys 0..4GB at KERNEL_VIRT_OFFSET before calling us.
    // Mark VGA as ready so write_byte uses it.
    READY.store(true, Ordering::Release);
    clear();
}

#[inline]
fn vga_addr() -> u64 {
    phys_to_virt(VGA_PHYS)
}

fn buf() -> *mut u16 {
    vga_addr() as *mut u16
}

pub fn clear() {
    if !READY.load(Ordering::Acquire) { return; }
    let blank = entry(b' ', COLOR.load(Ordering::Relaxed) as u8);
    for i in 0..(WIDTH * HEIGHT) {
        unsafe { buf().add(i).write_volatile(blank); }
    }
    COL.store(0, Ordering::Relaxed);
    ROW.store(0, Ordering::Relaxed);
}

pub fn write_byte(b: u8) {
    if !READY.load(Ordering::Acquire) { return; }
    match b {
        b'\r' => { COL.store(0, Ordering::Relaxed); }
        b'\n' => newline(),
        0x08  => backspace(),
        _     => put_char(b),
    }
    move_cursor();
}

pub fn write_str(s: &str) {
    for b in s.bytes() { write_byte(b); }
}

fn put_char(c: u8) {
    let col   = COL.load(Ordering::Relaxed);
    let row   = ROW.load(Ordering::Relaxed);
    let color = COLOR.load(Ordering::Relaxed) as u8;
    unsafe { buf().add(row * WIDTH + col).write_volatile(entry(c, color)); }
    if col + 1 >= WIDTH { newline(); }
    else                { COL.store(col + 1, Ordering::Relaxed); }
}

fn backspace() {
    let col = COL.load(Ordering::Relaxed);
    let row = ROW.load(Ordering::Relaxed);
    if col > 0 {
        let nc = col - 1;
        COL.store(nc, Ordering::Relaxed);
        let color = COLOR.load(Ordering::Relaxed) as u8;
        unsafe { buf().add(row * WIDTH + nc).write_volatile(entry(b' ', color)); }
    }
}

fn newline() {
    COL.store(0, Ordering::Relaxed);
    let row = ROW.load(Ordering::Relaxed);
    if row + 1 >= HEIGHT { scroll(); }
    else                 { ROW.store(row + 1, Ordering::Relaxed); }
}

fn scroll() {
    // Move rows 1..HEIGHT-1 up by one
    unsafe {
        let base = buf();
        for row in 1..HEIGHT {
            for col in 0..WIDTH {
                let src = base.add(row * WIDTH + col).read_volatile();
                base.add((row - 1) * WIDTH + col).write_volatile(src);
            }
        }
        let blank = entry(b' ', COLOR.load(Ordering::Relaxed) as u8);
        for col in 0..WIDTH {
            base.add((HEIGHT - 1) * WIDTH + col).write_volatile(blank);
        }
    }
    ROW.store(HEIGHT - 1, Ordering::Relaxed);
}

fn move_cursor() {
    let pos = ROW.load(Ordering::Relaxed) * WIDTH + COL.load(Ordering::Relaxed);
    unsafe {
        outb(0x3D4, 0x0F);
        outb(0x3D5, (pos & 0xFF) as u8);
        outb(0x3D4, 0x0E);
        outb(0x3D5, ((pos >> 8) & 0xFF) as u8);
    }
}

fn entry(c: u8, color: u8) -> u16 {
    (c as u16) | ((color as u16) << 8)
}

pub fn set_color(fg: u8, bg: u8) {
    COLOR.store(((bg & 0xF) << 4 | (fg & 0xF)) as usize, Ordering::Relaxed);
}
