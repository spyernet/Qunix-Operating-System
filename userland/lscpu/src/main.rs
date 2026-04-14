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


fn av(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i); let mut n=0; while *p.add(n)!=0{n+=1;}
        String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).to_string()
    }).collect()
}
fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> String {
    let mut d=alloc::vec![0u8;1<<20]; let mut t=0;
    loop{if t>=d.len(){d.resize(d.len()*2,0);} let n=read(fd,&mut d[t..]); if n<=0{break;} t+=n as usize;}
    String::from_utf8_lossy(&d[..t]).to_string()
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    let fd=open(b"/proc/cpuinfo\0",O_RDONLY,0);
    let mut model_name="x86_64".to_string(); let mut cores=1u32; let mut threads=1u32; let mut mhz="0.000".to_string();
    if fd>=0 {
        let s = {let mut d=alloc::vec![0u8;65536];let mut t=0;loop{let n=read(fd as i32,&mut d[t..]);if n<=0{break;}t+=n as usize;}String::from_utf8_lossy(&d[..t]).to_string()};
        close(fd as i32);
        for line in s.lines(){
            if line.starts_with("model name"){if let Some(v)=line.split(':').nth(1){model_name=v.trim().to_string();}}
            if line.starts_with("cpu MHz"){if let Some(v)=line.split(':').nth(1){mhz=v.trim().to_string();}}
            if line.starts_with("cpu cores"){if let Some(v)=line.split(':').nth(1){cores=v.trim().parse().unwrap_or(1);}}
            if line.starts_with("siblings"){if let Some(v)=line.split(':').nth(1){threads=v.trim().parse().unwrap_or(1);}}
        }
    }
    wstr(&alloc::format!("Architecture:            x86_64\n"));
    wstr(&alloc::format!("CPU op-mode(s):          32-bit, 64-bit\n"));
    wstr(&alloc::format!("Byte Order:              Little Endian\n"));
    wstr(&alloc::format!("CPU(s):                  {}\n", cores));
    wstr(&alloc::format!("Thread(s) per core:      1\n"));
    wstr(&alloc::format!("Core(s) per socket:      {}\n", cores));
    wstr(&alloc::format!("Socket(s):               1\n"));
    wstr(&alloc::format!("Model name:              {}\n", model_name));
    wstr(&alloc::format!("CPU MHz:                 {}\n", mhz));
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
