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


fn args_from_argv(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i);
        let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }).collect()
}

fn write_str(s: &str) { write(STDOUT, s.as_bytes()); }
fn write_err(s: &str) { write(STDERR, s.as_bytes()); }
fn cstr_str(p: *const u8) -> String {
    unsafe {
        let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }
}

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let args = args_from_argv(argc, argv);
    let mut show_lines = false;
    let mut number_nonblank = false;
    let mut show_ends = false;
    let mut show_tabs = false;
    let mut squeeze = false;
    let mut files: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-n" => show_lines = true,
            "-b" => number_nonblank = true,
            "-E" | "-e" => show_ends = true,
            "-T" | "-t" => show_tabs = true,
            "-s" => squeeze = true,
            "-A" => { show_ends = true; show_tabs = true; }
            "-v" => {}
            "--" => { files.extend(args[i+1..].iter().cloned()); break; }
            s if s.starts_with('-') && s.len() > 1 && !s.starts_with("--") => {
                for c in s[1..].chars() {
                    match c { 'n'=> show_lines=true, 'b'=> number_nonblank=true,
                              'E'|'e'=> show_ends=true, 'T'|'t'=> show_tabs=true,
                              's'=> squeeze=true, 'A'=> {show_ends=true; show_tabs=true;}, _ => {} }
                }
            }
            _ => files.push(args[i].clone()),
        }
        i += 1;
    }

    let simple = !show_lines && !number_nonblank && !show_ends && !show_tabs && !squeeze;

    if files.is_empty() {
        cat_fd(STDIN, simple, show_lines, number_nonblank, show_ends, show_tabs, squeeze, &mut 0);
    } else {
        let mut lineno = 0u64;
        for f in &files {
            if f == "-" {
                cat_fd(STDIN, simple, show_lines, number_nonblank, show_ends, show_tabs, squeeze, &mut lineno);
            } else {
                let mut path = f.clone(); path.push('\0');
                let fd = open(path.as_bytes(), O_RDONLY, 0);
                if fd < 0 { write_err(&alloc::format!("cat: {}: No such file or directory\n", f)); continue; }
                cat_fd(fd as i32, simple, show_lines, number_nonblank, show_ends, show_tabs, squeeze, &mut lineno);
                close(fd as i32);
            }
        }
    }
    exit(0)
}

fn cat_fd(fd: i32, simple: bool, lines: bool, nb: bool, ends: bool, tabs: bool, squeeze: bool, lineno: &mut u64) {
    if simple {
        let mut buf = [0u8; 65536];
        loop { let n = read(fd, &mut buf); if n <= 0 { break; } write(STDOUT, &buf[..n as usize]); }
        return;
    }
    let mut buf = [0u8; 65536];
    let mut prev_empty = false;
    let mut at_start = true;
    loop {
        let n = read(fd, &mut buf);
        if n <= 0 { break; }
        for &b in &buf[..n as usize] {
            if at_start {
                if squeeze && b == b'\n' { if prev_empty { continue; } prev_empty = true; } else { prev_empty = false; }
                if lines || nb {
                    let do_num = if nb { b != b'\n' } else { true };
                    if do_num { *lineno += 1; write_str(&alloc::format!("{:6}\t", lineno)); }
                }
                at_start = false;
            }
            if tabs && b == b'\t' { write(STDOUT, b"^I"); } 
            else if ends && b == b'\n' { write(STDOUT, b"$\n"); at_start = true; }
            else { write(STDOUT, &[b]); if b == b'\n' { at_start = true; } }
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
