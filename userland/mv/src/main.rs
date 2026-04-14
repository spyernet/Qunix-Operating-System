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


fn args_from_argv(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i);
        let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }).collect()
}
fn write_str(s: &str) { write(STDOUT, s.as_bytes()); }
fn write_err(s: &str) { write(STDERR, s.as_bytes()); }

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let args = args_from_argv(argc, argv);
    let mut force=false; let mut interactive=false; let mut no_clobber=false;
    let mut verbose=false; let mut update=false; let mut backup=false;
    let mut files: Vec<String>=Vec::new();
    let mut i=1;
    while i<args.len() {
        match args[i].as_str() {
            "-f"|"--force"=>force=true,"-i"|"--interactive"=>interactive=true,
            "-n"|"--no-clobber"=>no_clobber=true,"-v"|"--verbose"=>verbose=true,
            "-u"|"--update"=>update=true,"-b"|"--backup"=>backup=true,
            "--"=>{i+=1;files.extend(args[i..].iter().cloned());break;}
            s if s.starts_with('-')&&!s.starts_with("--")=>{for c in s[1..].chars(){match c{'f'=>force=true,'i'=>interactive=true,'n'=>no_clobber=true,'v'=>verbose=true,'u'=>update=true,'b'=>backup=true,_=>{}}}}
            _=>files.push(args[i].clone()),
        }
        i+=1;
    }
    if files.len()<2{write_err("mv: missing operand\n");exit(1);}
    let dst=files.last().unwrap().clone();
    let srcs=&files[..files.len()-1];
    let dst_is_dir={let mut p=dst.clone();p.push('\0');let mut st=[0u64;22];(unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)})==0&&(st[2]>>32)&0xF000==0x4000};
    for src in srcs {
        let dest=if dst_is_dir {let base=src.rsplit('/').next().unwrap_or(src);alloc::format!("{}/{}",dst,base)}else{dst.clone()};
        if verbose{write_str(&alloc::format!("'{}' -> '{}'\n",src,dest));}
        if no_clobber{let mut p=dest.clone();p.push('\0');let mut st=[0u64;22];if (unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)})==0{continue;}}
        if interactive{write_err(&alloc::format!("mv: overwrite '{}'? ",dest));let mut b=[0u8;4];read(STDIN,&mut b);if b[0]!=b'y'&&b[0]!=b'Y'{continue;}}
        if backup{let bk=alloc::format!("{}~",dest);let mut s=src.to_string();s.push('\0');let mut d=bk.clone();d.push('\0');unsafe{syscall::syscall2(82,s.as_ptr() as u64,d.as_ptr() as u64)};}
        let mut s=src.to_string();s.push('\0');let mut d=dest.clone();d.push('\0');
        if unsafe{syscall::syscall2(82,s.as_ptr() as u64,d.as_ptr() as u64)}<0{
            write_err(&alloc::format!("mv: cannot move '{}' to '{}'\n",src,dest));
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
