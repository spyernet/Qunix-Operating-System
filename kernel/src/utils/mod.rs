/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Qunix utility functions — alignment, bit ops, string helpers.

use core::fmt;

pub fn align_up(val: u64, align: u64) -> u64 {
    (val + align - 1) & !(align - 1)
}

pub fn align_down(val: u64, align: u64) -> u64 {
    val & !(align - 1)
}

pub fn is_power_of_two(n: u64) -> bool {
    n != 0 && (n & (n - 1)) == 0
}

pub fn min(a: usize, b: usize) -> usize {
    if a < b { a } else { b }
}

pub fn max(a: usize, b: usize) -> usize {
    if a > b { a } else { b }
}

pub struct RingBuffer<const N: usize> {
    buf: [u8; N],
    read: usize,
    write: usize,
    len: usize,
}

impl<const N: usize> RingBuffer<N> {
    pub const fn new() -> Self {
        RingBuffer { buf: [0u8; N], read: 0, write: 0, len: 0 }
    }

    pub fn push(&mut self, b: u8) -> bool {
        if self.len == N { return false; }
        self.buf[self.write] = b;
        self.write = (self.write + 1) % N;
        self.len += 1;
        true
    }

    pub fn pop(&mut self) -> Option<u8> {
        if self.len == 0 { return None; }
        let b = self.buf[self.read];
        self.read = (self.read + 1) % N;
        self.len -= 1;
        Some(b)
    }

    pub fn is_empty(&self) -> bool { self.len == 0 }
    pub fn is_full(&self) -> bool { self.len == N }
    pub fn len(&self) -> usize { self.len }
}

pub fn memset(dst: *mut u8, val: u8, count: usize) {
    unsafe { core::ptr::write_bytes(dst, val, count); }
}

pub fn memcpy(dst: *mut u8, src: *const u8, count: usize) {
    unsafe { core::ptr::copy_nonoverlapping(src, dst, count); }
}

pub fn memcmp(a: *const u8, b: *const u8, count: usize) -> i32 {
    for i in 0..count {
        let x = unsafe { *a.add(i) };
        let y = unsafe { *b.add(i) };
        if x != y { return x as i32 - y as i32; }
    }
    0
}
