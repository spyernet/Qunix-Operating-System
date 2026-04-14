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
    let mut delimiters="\t".to_string(); let mut serial=false;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-d"|"--delimiters"=>{ i+=1; delimiters=a.get(i).cloned().unwrap_or("\t".to_string()); }
            "-s"|"--serial"=>serial=true,
            s if s.starts_with("-d")=>delimiters=s[2..].to_string(),
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    if files.is_empty(){files.push("-".to_string());}
    let delims:Vec<char>=delimiters.chars().collect();
    let get_delim=|i:usize|->char{delims[i%delims.len()]};
    if serial {
        for (fi,f) in files.iter().enumerate(){
            let fd=if f=="-"{STDIN}else{let mut p=f.clone();p.push('\0');open(p.as_bytes(),O_RDONLY,0) as i32};
            if fd<0{continue;}
            let s=rdall(fd); if f!="-"{close(fd);}
            let lines:Vec<&str>=s.split('\n').filter(|l|!l.is_empty()).collect();
            for (i,l) in lines.iter().enumerate(){
                if i>0{let d=get_delim(i-1);if d=='\n'{write(STDOUT,b"\n");}else{write(STDOUT,d.to_string().as_bytes());}}
                wstr(l);
            }
            write(STDOUT,b"\n");
        }
    } else {
        // Read all files into memory
        let contents:Vec<Vec<String>>=files.iter().map(|f|{
            let fd=if f=="-"{STDIN}else{let mut p=f.clone();p.push('\0');open(p.as_bytes(),O_RDONLY,0) as i32};
            if fd<0{return Vec::new();}
            let s=rdall(fd);if f!="-"{close(fd);}
            s.split('\n').map(|l|l.to_string()).collect()
        }).collect();
        let max_rows=contents.iter().map(|c|c.len()).max().unwrap_or(0);
        for row in 0..max_rows {
            for (ci,col) in contents.iter().enumerate(){
                if ci>0{let d=get_delim(ci-1);write(STDOUT,d.to_string().as_bytes());}
                wstr(col.get(row).map(|s|s.as_str()).unwrap_or(""));
            }
            write(STDOUT,b"\n");
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
