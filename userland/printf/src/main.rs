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
    if a.len() < 2 { e("printf: missing operand\n"); exit(1); }
    let mut i = 1;
    // Skip --
    if a[i] == "--" { i += 1; }
    if i >= a.len() { exit(0); }
    let fmt = &a[i]; i += 1;
    let args = &a[i..];
    let out = fmt_printf(fmt, args);
    write(STDOUT, out.as_bytes());
    exit(0)
}

fn fmt_printf(fmt: &str, args: &[String]) -> String {
    let mut out = String::new();
    let mut ai = 0usize;
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            i += 1;
            if i < chars.len() {
                match chars[i] {
                    'n' => out.push('\n'), 't' => out.push('\t'),
                    'r' => out.push('\r'), 'a' => out.push('\x07'),
                    'b' => out.push('\x08'), 'f' => out.push('\x0c'),
                    'v' => out.push('\x0b'), '\\' => out.push('\\'),
                    '0' => out.push('\0'), 'e' | 'E' => out.push('\x1b'),
                    c => { out.push('\\'); out.push(c); }
                }
            }
        } else if chars[i] == '%' {
            i += 1;
            if i >= chars.len() { break; }
            // Flags: -, +, space, 0
            let mut flags = String::new();
            while i < chars.len() && "-+ 0#".contains(chars[i]) {
                flags.push(chars[i]); i += 1;
            }
            // Width
            let mut width = String::new();
            while i < chars.len() && chars[i].is_ascii_digit() { width.push(chars[i]); i += 1; }
            // Precision
            let mut prec = String::new();
            if i < chars.len() && chars[i] == '.' {
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() { prec.push(chars[i]); i += 1; }
            }
            if i >= chars.len() { break; }
            let spec = chars[i];
            let arg = args.get(ai).map(|s| s.as_str()).unwrap_or("");
            ai = (ai + 1).min(args.len());
            let w = width.parse::<usize>().unwrap_or(0);
            let left = flags.contains('-');
            let zero = flags.contains('0');
            match spec {
                's' => {
                    let s = if prec.is_empty() { arg.to_string() }
                            else { let p = prec.parse::<usize>().unwrap_or(arg.len()); arg.chars().take(p).collect() };
                    if left { out.push_str(&alloc::format!("{:<w$}", s, w=w)); }
                    else    { out.push_str(&alloc::format!("{:>w$}", s, w=w)); }
                }
                'd' | 'i' => {
                    let n = parse_int(arg);
                    if zero && !left { out.push_str(&alloc::format!("{:0>w$}", n, w=w)); }
                    else if left     { out.push_str(&alloc::format!("{:<w$}", n, w=w)); }
                    else             { out.push_str(&alloc::format!("{:>w$}", n, w=w)); }
                }
                'u' => { let n = parse_uint(arg);
                    out.push_str(&alloc::format!("{}", n)); }
                'o' => { let n = parse_uint(arg);
                    out.push_str(&alloc::format!("{:o}", n)); }
                'x' => { let n = parse_uint(arg);
                    out.push_str(&alloc::format!("{:x}", n)); }
                'X' => { let n = parse_uint(arg);
                    out.push_str(&alloc::format!("{:X}", n)); }
                'f' | 'F' => {
                    let n = arg.parse::<f64>().unwrap_or(0.0);
                    let p = prec.parse::<usize>().unwrap_or(6);
                    out.push_str(&fmt_float(n, p, w, left, zero));
                }
                'e' => {
                    let n = arg.parse::<f64>().unwrap_or(0.0);
                    out.push_str(&alloc::format!("{:e}", n));
                }
                'c' => { out.push(arg.chars().next().unwrap_or('\0')); }
                'b' => { out.push_str(&decode_escapes(arg)); ai -= 1; ai += 1; }
                'q' => { out.push('\''); out.push_str(arg); out.push('\''); }
                '%' => { out.push('%'); ai -= 1; }
                _ => { out.push('%'); out.push(spec); }
            }
        } else {
            out.push(chars[i]);
        }
        i += 1;
    }
    out
}

fn parse_int(s: &str) -> i64 {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or(s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).unwrap_or(0)
    } else if s.starts_with('0') && s.len() > 1 {
        i64::from_str_radix(&s[1..], 8).unwrap_or_else(|_| s.parse().unwrap_or(0))
    } else {
        s.parse().unwrap_or(0)
    }
}

fn parse_uint(s: &str) -> u64 {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix("0x").or(s.strip_prefix("0X")) {
        u64::from_str_radix(hex, 16).unwrap_or(0)
    } else { s.parse().unwrap_or(0) }
}

fn fmt_float(n: f64, prec: usize, width: usize, left: bool, zero: bool) -> String {
    // Manual float formatting since we have no std
    let neg = n < 0.0;
    let abs = if neg { -n } else { n };
    let int_part = abs as u64;
    let mut frac = abs - int_part as f64;
    let mut frac_digits = String::new();
    for _ in 0..prec {
        frac *= 10.0;
        let d = frac as u8;
        frac_digits.push((b'0' + d) as char);
        frac -= d as f64;
    }
    let s = if prec == 0 { alloc::format!("{}{}", if neg{"-"}else{""}, int_part) }
            else { alloc::format!("{}{}.{}", if neg{"-"}else{""}, int_part, frac_digits) };
    if left      { alloc::format!("{:<w$}", s, w=width) }
    else if zero { alloc::format!("{:0>w$}", s, w=width) }
    else         { alloc::format!("{:>w$}", s, w=width) }
}

fn decode_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' { out.push(c); continue; }
        match it.next() {
            Some('n') => out.push('\n'), Some('t') => out.push('\t'),
            Some('r') => out.push('\r'), Some('\\') => out.push('\\'),
            Some('0') => out.push('\0'), Some('a') => out.push('\x07'),
            Some('b') => out.push('\x08'), Some(c) => { out.push('\\'); out.push(c); }
            None => out.push('\\'),
        }
    }
    out
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
