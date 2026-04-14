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
    let mut sep="\n".to_string();let mut equal_width=false;let mut fmt_s: Option<String>=None;
    let mut nums: Vec<String>=Vec::new();let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-s"|"--separator"=>{i+=1;sep=a.get(i).cloned().unwrap_or("\n".to_string());}
            "-w"|"--equal-width"=>equal_width=true,
            "-f"|"--format"=>{i+=1;fmt_s=a.get(i).cloned();}
            s if s.starts_with("-s")=>{sep=s[2..].to_string();}
            s if !s.starts_with('-')=>{nums.push(s.to_string());}
            _=>{}
        }
        i+=1;
    }
    let (first,step,last)=match nums.len(){
        0=>{werr("seq: missing operand\n");exit(1);}
        1=>(1.0f64,1.0f64,nums[0].parse::<f64>().unwrap_or(1.0)),
        2=>(nums[0].parse::<f64>().unwrap_or(1.0),1.0f64,nums[1].parse::<f64>().unwrap_or(1.0)),
        _=>(nums[0].parse::<f64>().unwrap_or(1.0),nums[1].parse::<f64>().unwrap_or(1.0),nums[2].parse::<f64>().unwrap_or(1.0)),
    };
    let max_w=if equal_width{last.to_string().len()}else{0};
    let mut x=first;let mut first_iter=true;
    while (step>0.0&&x<=last+1e-10)||(step<0.0&&x>=last-1e-10) {
        if !first_iter{write(STDOUT,sep.as_bytes());}first_iter=false;
        let s=if x.abs()<1e15 && x == (x as i64) as f64 {(x as i64).to_string()}else{alloc::format!("{:.6}",x)};
        if equal_width&&s.len()<max_w{for _ in 0..(max_w-s.len()){write(STDOUT,b"0");}}
        wstr(&s);
        x+=step;if x.is_infinite(){break;}
    }
    if !first_iter{write(STDOUT,b"\n");}
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
