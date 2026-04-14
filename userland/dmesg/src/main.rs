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


fn args(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i); let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }).collect()
}
fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> String {
    let mut d = alloc::vec![0u8; 1<<20]; let mut t = 0;
    loop { if t >= d.len() { d.resize(d.len()*2,0); } let n = read(fd, &mut d[t..]); if n<=0{break;} t+=n as usize; }
    String::from_utf8_lossy(&d[..t]).to_string()
}
fn rdfile(p: &str) -> String {
    let mut pa = p.to_string(); pa.push('\0');
    let fd = open(pa.as_bytes(), O_RDONLY, 0); if fd < 0 { return String::new(); }
    let s = rdall(fd as i32); close(fd as i32); s
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a=args(argc,argv);
    let mut clear=false;let mut follow=false;let mut human=false;let mut level: Option<u32>=None;
    let mut i=1;while i<a.len(){
        match a[i].as_str(){"-C"|"--clear"=>clear=true,"-w"|"--follow"=>follow=true,"-H"|"--human"=>human=true,"-l"|"--level"=>{i+=1;},"--color"=>{},"--nocolor"=>{},_=>{}}
        i+=1;
    }
    // Read kernel ring buffer via /proc/kmsg or syslog syscall
    let fd=open(b"/proc/kmsg\0",O_RDONLY,0);
    if fd>=0{
        let mut buf=[0u8;65536];
        let n=read(fd as i32,&mut buf);
        if n>0{write(STDOUT,&buf[..n as usize]);}
        close(fd as i32);
    } else {
        // Fallback: try /dev/kmsg
        let fd2=open(b"/dev/kmsg\0",O_RDONLY,0);
        if fd2>=0{
            let mut buf=[0u8;65536];
            let n=read(fd2 as i32,&mut buf);
            if n>0{write(STDOUT,&buf[..n as usize]);}
            close(fd2 as i32);
        } else {
            // Use syslog syscall (103)
            let mut buf=alloc::vec![0u8;65536];
            let n=unsafe{syscall::syscall3(103,3u64,buf.as_mut_ptr() as u64,buf.len() as u64)};
            if n>0{write(STDOUT,&buf[..n as usize]);}
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
