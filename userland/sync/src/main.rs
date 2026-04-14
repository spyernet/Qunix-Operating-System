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
    if a.iter().any(|s| s=="--help") { w("Usage: sync [--data] [FILE]...\nFlush file system buffers.\n"); exit(0); }
    if a.iter().any(|s| s=="--version") { w("sync (Qunix) 1.0\n"); exit(0); }
    let data_only = a.iter().any(|s| s=="--data");
    let files: Vec<&str> = a[1..].iter().filter(|s| !s.starts_with('-')).map(|s|s.as_str()).collect();
    if files.is_empty() {
        unsafe { syscall::syscall0(SYS_SYNC) };
    } else {
        for f in &files {
            let mut p=f.to_string(); p.push('\0');
            let fd=open(p.as_bytes(),O_RDONLY,0);
            if fd<0 { e(&alloc::format!("sync: {}: No such file\n",f)); continue; }
            if data_only {
                unsafe { syscall::syscall1(75, fd as u64) }; // fdatasync
            } else {
                unsafe { syscall::syscall1(74, fd as u64) }; // fsync
            }
            close(fd as i32);
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
