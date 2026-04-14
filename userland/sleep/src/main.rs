/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;


fn parse_argv(argc: u64, ap: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *ap.add(i); let mut n=0; while *p.add(n)!=0{n+=1;}
        String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).into_owned()
    }).collect()
}
fn w(s: &str) { write(STDOUT, s.as_bytes()); }
fn e(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> alloc::vec::Vec<u8> {
    let mut d=alloc::vec![0u8;1<<20]; let mut t=0;
    loop { if t>=d.len(){d.resize(d.len()*2,0);} let n=read(fd,&mut d[t..]); if n<=0{break;} t+=n as usize; }
    d.truncate(t); d
}
fn rdfile(path: &str) -> alloc::vec::Vec<u8> {
    let mut p=path.to_string(); p.push('\0');
    let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{return alloc::vec![];}
    let d=rdall(fd as i32); close(fd as i32); d
}
fn cstr(p: *const u8) -> String {
    unsafe { let mut n=0; while *p.add(n)!=0{n+=1;}
    String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).into_owned() }
}

#[no_mangle] #[link_section=".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = parse_argv(argc, argv);
    if a.len() < 2 {
        e("sleep: missing operand\n");
        w("Usage: sleep NUMBER[SUFFIX]...\nSUFFIX: s(seconds) m(minutes) h(hours) d(days)\n");
        exit(1);
    }
    if a.iter().any(|s| s=="--help") { w("Usage: sleep NUMBER[SUFFIX]...\n"); exit(0); }
    if a.iter().any(|s| s=="--version") { w("sleep (Qunix) 1.0\n"); exit(0); }

    let mut total_ns: i64 = 0;
    for arg in &a[1..] {
        if arg.starts_with('-') && arg.len() > 1 { e(&alloc::format!("sleep: invalid time interval '{}'\n", arg)); exit(1); }
        let s = arg.as_str().trim();
        let last = s.chars().last().unwrap_or('s');
        let (num_str, mult): (&str, i64) = if last.is_alphabetic() {
            (&s[..s.len()-1], match last { 'm'=>60, 'h'=>3600, 'd'=>86400, _=>1 })
        } else { (s, 1) };
        if num_str.is_empty() { e(&alloc::format!("sleep: invalid time interval '{}'\n", arg)); exit(1); }
        // Parse as rational: split on '.'
        let ns = if let Some(dot) = num_str.find('.') {
            let int_part: i64 = num_str[..dot].parse().unwrap_or(0);
            let frac_str = &num_str[dot+1..];
            let frac_digits = frac_str.len().min(9);
            let frac_val: i64 = frac_str[..frac_digits].parse().unwrap_or(0);
            let frac_ns = frac_val * 10i64.pow((9 - frac_digits) as u32);
            int_part * 1_000_000_000 + frac_ns
        } else {
            num_str.parse::<i64>().unwrap_or(0) * 1_000_000_000
        };
        total_ns += ns * mult;
    }

    if total_ns < 0 { e("sleep: invalid time interval\n"); exit(1); }
    let ts = [total_ns / 1_000_000_000, total_ns % 1_000_000_000];
    unsafe { syscall::syscall2(SYS_NANOSLEEP, ts.as_ptr() as u64, 0) };
    exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
