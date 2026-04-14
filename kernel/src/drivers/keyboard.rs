/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! PS/2 keyboard driver with line-discipline integration.
//!
//! Tracks Shift, Ctrl, Alt modifier state.
//! Translates scancodes to ASCII.
//! Special keys:
//!   Ctrl+C  → tty_input_byte(0x03) → SIGINT to foreground pgrp
//!   Ctrl+Z  → tty_input_byte(0x1A) → SIGTSTP
//!   Ctrl+D  → tty_input_byte(0x04) → EOF
//!   Ctrl+U  → tty_input_byte(0x15) → kill line
//! All input goes through the TTY line discipline (tty::tty_input_byte),
//! which handles echo, canonical buffering, and waking blocked readers.

use crate::arch::x86_64::port::inb;

const SCANCODE_MAP: [u8; 128] = [
    0,   27,  b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8',
    b'9',b'0', b'-', b'=', 8,   b'\t',b'q', b'w', b'e', b'r',
    b't',b'y', b'u', b'i', b'o', b'p', b'[', b']', b'\n', 0,
    b'a',b's', b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b'\'',b'`',0,   b'\\',b'z', b'x', b'c', b'v', b'b', b'n',
    b'm',b',', b'.', b'/', 0,   b'*', 0,   b' ', 0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   b'-',0,   0,   0,
    b'+',0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,
];

const SCANCODE_MAP_SHIFT: [u8; 128] = [
    0,   27,  b'!', b'@', b'#', b'$', b'%', b'^', b'&', b'*',
    b'(',b')', b'_', b'+', 8,   b'\t',b'Q', b'W', b'E', b'R',
    b'T',b'Y', b'U', b'I', b'O', b'P', b'{', b'}', b'\n', 0,
    b'A',b'S', b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'"',b'~', 0,   b'|', b'Z', b'X', b'C', b'V', b'B', b'N',
    b'M',b'<', b'>', b'?', 0,   b'*', 0,   b' ', 0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   b'-',0,   0,   0,
    b'+',0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,   0,   0,
    0,   0,   0,   0,   0,   0,   0,   0,
];

// Ctrl+key → ASCII control character (base letter scancode → byte)
// ctrl_table[scancode] = ASCII control char (or 0 if not applicable)
// We compute it: Ctrl+A=1, Ctrl+B=2, ..., Ctrl+Z=26
// Scancode for 'a'=0x1E, 'b'=0x30 etc. — easier to map by ASCII letter.
fn ctrl_char(ascii: u8) -> u8 {
    match ascii {
        b'a'..=b'z' => ascii - b'a' + 1,
        b'A'..=b'Z' => ascii - b'A' + 1,
        b'['        => 0x1B, // ESC
        b'\\'       => 0x1C,
        b']'        => 0x1D,
        b'^'        => 0x1E,
        b'_'        => 0x1F,
        _           => 0,
    }
}

use core::sync::atomic::{AtomicBool, Ordering};
static SHIFT_DOWN: AtomicBool = AtomicBool::new(false);
static CTRL_DOWN:  AtomicBool = AtomicBool::new(false);
static ALT_DOWN:   AtomicBool = AtomicBool::new(false);

pub fn init() {
    crate::drivers::irq::register(1, keyboard_irq_handler);
}

pub fn keyboard_irq_handler(_frame: &crate::arch::x86_64::interrupts::InterruptFrame) {
    let scancode = unsafe { inb(0x60) };

    // ── Modifier key tracking ─────────────────────────────────────────
    match scancode {
        // Left/Right Shift make/break
        0x2A | 0x36 => { SHIFT_DOWN.store(true,  Ordering::Relaxed); return; }
        0xAA | 0xB6 => { SHIFT_DOWN.store(false, Ordering::Relaxed); return; }
        // Left Ctrl make/break
        0x1D        => { CTRL_DOWN.store(true,   Ordering::Relaxed); return; }
        0x9D        => { CTRL_DOWN.store(false,  Ordering::Relaxed); return; }
        // Left Alt make/break
        0x38        => { ALT_DOWN.store(true,    Ordering::Relaxed); return; }
        0xB8        => { ALT_DOWN.store(false,   Ordering::Relaxed); return; }
        // Break codes for non-modifier keys — ignore
        0x80..=0xFF => return,
        _ => {}
    }

    if scancode as usize >= SCANCODE_MAP.len() { return; }

    let shift = SHIFT_DOWN.load(Ordering::Relaxed);
    let ctrl  = CTRL_DOWN.load(Ordering::Relaxed);

    let ascii = if shift {
        SCANCODE_MAP_SHIFT[scancode as usize]
    } else {
        SCANCODE_MAP[scancode as usize]
    };

    if ascii == 0 { return; }

    // ── Ctrl modifier ─────────────────────────────────────────────────
    let byte = if ctrl {
        let cc = ctrl_char(ascii);
        if cc == 0 { ascii } else { cc }
    } else {
        ascii
    };

    // Feed through TTY line discipline
    crate::tty::tty_input_byte(byte);
}

/// Raw read_char — only used by serial fallback (legacy, kept for compatibility).
/// New code should use tty::tty_read.
pub fn read_char() -> Option<char> { None }
