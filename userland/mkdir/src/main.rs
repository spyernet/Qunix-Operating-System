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
    let mut parents=false; let mut verbose=false; let mut mode=0o755u32; let mut dirs: Vec<String>=Vec::new();
    let mut i=1;
    while i<args.len() {
        match args[i].as_str() {
            "-p"|"--parents"=>parents=true,"-v"|"--verbose"=>verbose=true,
            "-m"|"--mode"=>{i+=1;mode=u32::from_str_radix(args.get(i).map(|s| s.as_str()).unwrap_or("755"),8).unwrap_or(0o755);}
            "--"=>{i+=1;dirs.extend(args[i..].iter().cloned());break;}
            s if s.starts_with("-m")=>{mode=u32::from_str_radix(&s[2..],8).unwrap_or(0o755);}
            s if s.starts_with('-')=>{for c in s[1..].chars(){match c{'p'=>parents=true,'v'=>verbose=true,_=>{}}}}
            _=>dirs.push(args[i].clone()),
        }
        i+=1;
    }
    let mut status=0i32;
    for d in &dirs {
        if parents {
            let parts: Vec<&str>=d.split('/').filter(|s|!s.is_empty()).collect();
            let mut path=if d.starts_with('/'){"".to_string()}else{".".to_string()};
            for part in parts{
                path=if path.is_empty()||path=="."{ part.to_string()}else{alloc::format!("{}/{}",path,part)};
                let mut p=path.clone();p.push('\0');
                let r=unsafe{syscall::syscall2(83,p.as_ptr() as u64,mode as u64)};
                if r>=0&&verbose{write_str(&alloc::format!("mkdir: created directory '{}'\n",&path));}
            }
        } else {
            let mut p=d.clone();p.push('\0');
            let r=unsafe{syscall::syscall2(83,p.as_ptr() as u64,mode as u64)};
            if r<0{write_err(&alloc::format!("mkdir: cannot create directory '{}': File exists\n",d));status=1;}
            else if verbose{write_str(&alloc::format!("mkdir: created directory '{}'\n",d));}
        }
    }
    exit(status)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
