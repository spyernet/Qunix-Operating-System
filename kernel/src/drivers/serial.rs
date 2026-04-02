use crate::arch::x86_64::port::{inb, outb};

const COM1: u16 = 0x3F8;

pub fn init() {
    unsafe {
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x80);
        outb(COM1 + 0, 0x03);
        outb(COM1 + 1, 0x00);
        outb(COM1 + 3, 0x03);
        outb(COM1 + 2, 0xC7);
        outb(COM1 + 4, 0x0B);
    }
}

fn is_transmit_empty() -> bool {
    unsafe { inb(COM1 + 5) & 0x20 != 0 }
}

fn received() -> bool {
    unsafe { inb(COM1 + 5) & 1 != 0 }
}

pub fn write_byte(b: u8) {
    while !is_transmit_empty() {}
    unsafe { outb(COM1, b); }
}

pub fn read_byte() -> Option<u8> {
    if received() {
        Some(unsafe { inb(COM1) })
    } else {
        None
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() {
        if b == b'\n' { write_byte(b'\r'); }
        write_byte(b);
    }
}
