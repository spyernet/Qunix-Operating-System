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
    let mut decode=false; let mut wrap=76usize; let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-d"|"--decode"=>decode=true,
            "-w"|"--wrap"=>{ i+=1; wrap=a.get(i).and_then(|s|s.parse().ok()).unwrap_or(76); }
            "-i"|"--ignore-garbage"=>{},
            s if s.starts_with("-w")=>wrap=s[2..].parse().unwrap_or(76),
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    let mut data=Vec::new();
    let load=|fd:i32,data:&mut Vec<u8>|{let mut buf=[0u8;65536];loop{let n=read(fd,&mut buf);if n<=0{break;}data.extend_from_slice(&buf[..n as usize]);}};
    if files.is_empty(){load(STDIN,&mut data);}
    else{for f in &files{if f=="-"{load(STDIN,&mut data);}else{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{continue;}load(fd as i32,&mut data);close(fd as i32);}}}
    if decode {
        let b64_chars=b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let clean:Vec<u8>=data.iter().filter(|&&b|b!=b'\n'&&b!=b'\r'&&b!=b' ').copied().collect();
        let mut i=0;
        while i+3<clean.len(){
            let a_=b64val(clean[i]);let b_=b64val(clean[i+1]);let c_=b64val(clean[i+2]);let d_=b64val(clean[i+3]);
            if a_<64&&b_<64{write(STDOUT,&[(a_<<2)|(b_>>4)]);}
            if c_<64{write(STDOUT,&[((b_&0xF)<<4)|(c_>>2)]);}
            if d_<64{write(STDOUT,&[((c_&3)<<6)|d_]);}
            i+=4;
        }
    } else {
        const CHARS:&[u8]=b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out=Vec::new(); let mut col=0usize;
        for chunk in data.chunks(3){
            let v=match chunk.len(){
                3=>((chunk[0] as u32)<<16)|((chunk[1] as u32)<<8)|(chunk[2] as u32),
                2=>((chunk[0] as u32)<<16)|((chunk[1] as u32)<<8),
                _=>(chunk[0] as u32)<<16,
            };
            out.push(CHARS[((v>>18)&63)as usize]);
            out.push(CHARS[((v>>12)&63)as usize]);
            out.push(if chunk.len()>1{CHARS[((v>>6)&63)as usize]}else{b'='});
            out.push(if chunk.len()>2{CHARS[(v&63)as usize]}else{b'='});
            if wrap>0{col+=4;if col>=wrap{out.push(b'\n');col=0;}}
        }
        if wrap>0&&col>0{out.push(b'\n');}
        write(STDOUT,&out);
    }
    exit(0)
}
fn b64val(b:u8)->u8{match b{b'A'..=b'Z'=>b-b'A',b'a'..=b'z'=>26+(b-b'a'),b'0'..=b'9'=>52+(b-b'0'),b'+'=>62,b'/'=>63,_=>64}}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
