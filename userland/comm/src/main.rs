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
    let mut suppress=[false;3]; let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-1"=>suppress[0]=true,"-2"=>suppress[1]=true,"-3"=>suppress[2]=true,
            "-12"=>{suppress[0]=true;suppress[1]=true;}
            "-13"=>{suppress[0]=true;suppress[2]=true;}
            "-23"=>{suppress[1]=true;suppress[2]=true;}
            "-123"=>{suppress[0]=true;suppress[1]=true;suppress[2]=true;}
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    if files.len()<2{werr("comm: missing operand\n");exit(1);}
    let read_sorted=|f:&str|->Vec<String>{
        let fd=if f=="-"{STDIN}else{let mut p=f.to_string();p.push('\0');open(p.as_bytes(),O_RDONLY,0) as i32};
        if fd<0{return Vec::new();}
        let s=rdall(fd);if f!="-"{close(fd);}
        s.split('\n').filter(|l|!l.is_empty()).map(|l|l.to_string()).collect()
    };
    let a_lines=read_sorted(&files[0]); let b_lines=read_sorted(&files[1]);
    let mut ai=0; let mut bi=0;
    while ai<a_lines.len()||bi<b_lines.len(){
        let cmp=match(a_lines.get(ai),b_lines.get(bi)){
            (Some(a),Some(b))=>a.cmp(b),
            (Some(_),None)=>core::cmp::Ordering::Less,
            (None,Some(_))=>core::cmp::Ordering::Greater,
            (None,None)=>break,
        };
        match cmp{
            core::cmp::Ordering::Less=>{if !suppress[0]{wstr(&a_lines[ai]);write(STDOUT,b"\n");}ai+=1;}
            core::cmp::Ordering::Greater=>{if !suppress[1]{wstr("\t");wstr(&b_lines[bi]);write(STDOUT,b"\n");}bi+=1;}
            core::cmp::Ordering::Equal=>{if !suppress[2]{wstr("\t\t");wstr(&a_lines[ai]);write(STDOUT,b"\n");}ai+=1;bi+=1;}
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
