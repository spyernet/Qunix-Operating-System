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
    let mut no_newline=false;let mut interpret=false;let mut start=1;
    loop{match a.get(start).map(|s|s.as_str()){
        Some("-n")=>{no_newline=true;start+=1;}
        Some("-e")=>{interpret=true;start+=1;}
        Some("-E")=>{interpret=false;start+=1;}
        Some(s) if s.starts_with('-')&&s[1..].chars().all(|c|"neE".contains(c))=>{
            for c in s[1..].chars(){match c{'n'=>no_newline=true,'e'=>interpret=true,'E'=>interpret=false,_=>{}}}
            start+=1;
        }
        _=>break,
    }}
    let out=a[start..].join(" ");
    if interpret{let d=decode_esc(&out);write(STDOUT,d.as_bytes());}
    else{write(STDOUT,out.as_bytes());}
    if !no_newline{write(STDOUT,b"\n");}
    exit(0)
}
fn decode_esc(s:&str)->String{
    let mut o=String::new();let mut it=s.chars();
    while let Some(c)=it.next(){if c!='\\'{ o.push(c);continue;}
        match it.next(){Some('n')=>o.push('\n'),Some('t')=>o.push('\t'),Some('r')=>o.push('\r'),Some('a')=>o.push('\x07'),Some('b')=>o.push('\x08'),Some('\\')=>o.push('\\'),Some('0')=>o.push('\0'),Some('e')|Some('E')=>o.push('\x1B'),Some(c)=>{o.push('\\');o.push(c);},None=>o.push('\\')}
    }
    o
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
