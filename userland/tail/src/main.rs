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
    let mut n_lines: i64 = 10; let mut n_bytes: i64 = -1; let mut follow=false;
    let mut quiet=false; let mut verbose=false; let mut from_start=false;
    let mut files: Vec<String> = Vec::new();
    let mut i=1;
    while i < args.len() {
        match args[i].as_str() {
            "-f"|"--follow" => follow=true, "-F" => follow=true,
            "-q"|"--quiet"|"--silent" => quiet=true, "-v"|"--verbose" => verbose=true,
            "-n"|"--lines" => { i+=1; if i<args.len() { let s=&args[i]; if s.starts_with('+'){from_start=true;n_lines=s[1..].parse().unwrap_or(0);}else{n_lines=s.parse().unwrap_or(10);} } }
            "-c"|"--bytes" => { i+=1; if i<args.len() { n_bytes=args[i].parse().unwrap_or(-1); } }
            s if s.starts_with("-n") => { let s2=&s[2..]; if s2.starts_with('+'){from_start=true;n_lines=s2[1..].parse().unwrap_or(0);}else{n_lines=s2.parse().unwrap_or(10);} }
            s if s.starts_with("-c") => { n_bytes=s[2..].parse().unwrap_or(-1); }
            s if s.starts_with('-') && s[1..].chars().all(|c| c.is_ascii_digit()) => { n_lines=s[1..].parse().unwrap_or(10); }
            _ => files.push(args[i].clone()),
        }
        i+=1;
    }

    let do_tail = |fd:i32, name:&str| {
        let mut data=alloc::vec![0u8;1<<20]; let mut tot=0;
        loop{let n=read(fd,&mut data[tot..]); if n<=0{break;} tot+=n as usize; if tot>=data.len(){data.resize(data.len()*2,0);}}
        let text=String::from_utf8_lossy(&data[..tot]).to_string();
        let lines: Vec<&str>=text.split('\n').filter(|l| !l.is_empty()).collect();
        let start = if from_start { (n_lines as usize).saturating_sub(1) }
                    else { lines.len().saturating_sub(n_lines as usize) };
        for l in &lines[start..] { write(STDOUT,l.as_bytes()); write(STDOUT,b"\n"); }
    };

    let multiple=files.len()>1;
    if files.is_empty() { do_tail(STDIN,"(stdin)"); }
    else { for (idx,f) in files.iter().enumerate() {
        if multiple||verbose { if !quiet { write_str(&alloc::format!("==> {} <==\n",f)); } }
        if f=="-"{do_tail(STDIN,"-");}
        else{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{write_err(&alloc::format!("tail: {}: cannot open\n",f));continue;}do_tail(fd as i32,f);close(fd as i32);}
        if multiple&&!quiet&&idx+1<files.len(){write(STDOUT,b"\n");}
    } }

    if follow && !files.is_empty() {
        // Follow mode: poll for changes
        let mut last_size = alloc::vec![0i64; files.len()];
        loop {
            nanosleep_ms(100);
            for (idx,f) in files.iter().enumerate() {
                let mut st = [0u64; 22]; let mut p=f.clone(); p.push('\0');
                unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)};
                let size = st[7] as i64;
                if size > last_size[idx] {
                    let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{continue;}
                    unsafe{syscall::syscall3(8,fd as u64,last_size[idx] as u64,0)};
                    let mut buf=[0u8;4096];
                    loop{let n=read(fd as i32,&mut buf); if n<=0{break;} write(STDOUT,&buf[..n as usize]);}
                    close(fd as i32);
                    last_size[idx]=size;
                }
            }
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
