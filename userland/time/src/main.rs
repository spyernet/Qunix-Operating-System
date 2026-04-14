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
    if a.len()<2{werr("usage: time command\n");exit(1);}
    let cmd=&a[1..];
    let mut ts=[0i64;2];clock_gettime(1,&mut ts);
    let pid=fork();
    if pid==0{
        let av: Vec<String>=cmd.iter().map(|s|{let mut x=s.clone();x.push('\0');x}).collect();
        let av2: Vec<*const u8>=av.iter().map(|s|s.as_ptr() as *const u8).chain(core::iter::once(core::ptr::null())).collect();
        let ep:[*const u8;1]=[core::ptr::null()];
        let mut c=cmd[0].clone();c.push('\0');
        execve(c.as_bytes(),&av2,&ep);exit(127);
    }
    let mut st=0i32;
    if pid>0{waitpid(pid as i32,&mut st,0);}
    let mut te=[0i64;2];clock_gettime(1,&mut te);
    let diff=(te[0]-ts[0])*1000+(te[1]-ts[1])/1_000_000;
    werr(&alloc::format!("\nreal\t{}m{:.3}s\nuser\t0m0.000s\nsys\t0m0.000s\n",diff/60000,(diff%60000)as f64/1000.0));
    exit(if st&0x7F==0{(st>>8)&0xFF}else{128+(st&0x7F)})
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
