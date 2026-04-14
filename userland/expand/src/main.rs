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
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = av(argc, argv);
    let mut tabstops=8usize; let mut initial_only=false;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-t"|"--tabs"=>{ i+=1; tabstops=a.get(i).and_then(|s|s.parse().ok()).unwrap_or(8); }
            "-i"|"--initial"=>initial_only=true,
            s if s.starts_with("-t")=>tabstops=s[2..].parse().unwrap_or(8),
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    let process=|fd:i32|{
        let s=rdall(fd);
        for line in s.split('\n'){
            let mut col=0usize; let mut in_initial=true;
            for c in line.chars(){
                if c=='\t'&&(!initial_only||in_initial){
                    let spaces=tabstops-(col%tabstops);
                    for _ in 0..spaces{write(STDOUT,b" ");col+=1;}
                }else{
                    if c!=' '&&c!='\t'{in_initial=false;}
                    write(STDOUT,c.to_string().as_bytes());
                    col+=1;
                }
            }
            write(STDOUT,b"\n");
        }
    };
    if files.is_empty(){process(STDIN);}
    else{for f in &files{if f=="-"{process(STDIN);}else{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{continue;}process(fd as i32);close(fd as i32);}}}
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
