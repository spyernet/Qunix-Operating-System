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
    let mut input_file=String::new(); let mut output_file=String::new();
    let mut bs=512usize; let mut count: Option<u64>=None; let mut skip=0u64; let mut seek=0u64;
    let mut conv=String::new(); let mut status="noxfer".to_string();
    for arg in &a[1..]{
        if let Some(v)=arg.strip_prefix("if="){input_file=v.to_string();}
        else if let Some(v)=arg.strip_prefix("of="){output_file=v.to_string();}
        else if let Some(v)=arg.strip_prefix("bs="){bs=parse_size(v);}
        else if let Some(v)=arg.strip_prefix("ibs="){bs=parse_size(v);}
        else if let Some(v)=arg.strip_prefix("obs="){bs=parse_size(v);}
        else if let Some(v)=arg.strip_prefix("count="){count=v.parse().ok();}
        else if let Some(v)=arg.strip_prefix("skip="){skip=v.parse().unwrap_or(0);}
        else if let Some(v)=arg.strip_prefix("seek="){seek=v.parse().unwrap_or(0);}
        else if let Some(v)=arg.strip_prefix("conv="){conv=v.to_string();}
        else if let Some(v)=arg.strip_prefix("status="){status=v.to_string();}
    }
    let in_fd=if input_file.is_empty()||input_file=="-"{STDIN}else{let mut p=input_file.clone();p.push('\0');open(p.as_bytes(),O_RDONLY,0) as i32};
    let out_fd=if output_file.is_empty()||output_file=="-"{STDOUT}else{let mut p=output_file.clone();p.push('\0');open(p.as_bytes(),O_WRONLY|O_CREAT|O_TRUNC,0o666) as i32};
    if in_fd<0{werr("dd: cannot open input\n");exit(1);}
    if out_fd<0{werr("dd: cannot open output\n");exit(1);}
    if skip>0{unsafe{syscall::syscall3(8,in_fd as u64,(skip*bs as u64),0)};}
    if seek>0{unsafe{syscall::syscall3(8,out_fd as u64,(seek*bs as u64),0)};}
    let mut buf=alloc::vec![0u8;bs]; let mut blocks=0u64; let mut bytes=0u64;
    loop{
        if let Some(c)=count{if blocks>=c{break;}}
        let n=read(in_fd,&mut buf); if n<=0{break;}
        let mut chunk=&buf[..n as usize];
        let mut out_chunk=chunk.to_vec();
        if conv.contains("ucase"){for b in &mut out_chunk{*b=b.to_ascii_uppercase();}}
        if conv.contains("lcase"){for b in &mut out_chunk{*b=b.to_ascii_lowercase();}}
        if conv.contains("swab")&&out_chunk.len()>=2{for i in (0..out_chunk.len()-1).step_by(2){out_chunk.swap(i,i+1);}}
        write(out_fd,&out_chunk);
        bytes+=n as u64; blocks+=1;
    }
    if in_fd!=STDIN{close(in_fd);}
    if out_fd!=STDOUT{close(out_fd);}
    if status!="none"{
        werr(&alloc::format!("{0}+0 records in\n{0}+0 records out\n{1} bytes ({2}) copied\n",blocks,bytes,fmt_bytes(bytes)));
    }
    exit(0)
}
fn parse_size(s: &str) -> usize {
    let last=s.chars().last().unwrap_or('0');
    let mult=match last.to_uppercase().to_string().as_str(){"K"=>1024,"M"=>1048576,"G"=>1073741824,_=>1};
    let n=if last.is_alphabetic(){s[..s.len()-1].parse().unwrap_or(512)}else{s.parse().unwrap_or(512)};
    n*mult
}
fn fmt_bytes(b: u64) -> String {
    if b<1024{alloc::format!("{} B",b)}
    else if b<1048576{alloc::format!("{:.2} kB",b as f64/1024.0)}
    else if b<1073741824{alloc::format!("{:.2} MB",b as f64/1048576.0)}
    else{alloc::format!("{:.2} GB",b as f64/1073741824.0)}
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
