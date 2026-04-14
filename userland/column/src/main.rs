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
    let mut sep='\t'; let mut table=false; let mut fillrows=false; let mut width=80usize;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-t"|"--table"=>table=true,
            "-s"|"--separator"=>{ i+=1; sep=a.get(i).and_then(|s|s.chars().next()).unwrap_or('\t'); }
            "-x"|"--fillrows"=>fillrows=true,
            "-c"|"--output-width"=>{ i+=1; width=a.get(i).and_then(|s|s.parse().ok()).unwrap_or(80); }
            s if s.starts_with("-s")=>sep=s[2..].chars().next().unwrap_or('\t'),
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    let mut lines=Vec::new();
    let process=|fd:i32,lines:&mut Vec<String>|{
        let s=rdall(fd);
        for l in s.split('\n'){ if !l.is_empty(){lines.push(l.to_string());} }
    };
    if files.is_empty(){process(STDIN,&mut lines);}
    else{for f in &files{if f=="-"{process(STDIN,&mut lines);}else{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{continue;}process(fd as i32,&mut lines);close(fd as i32);}}}

    if table {
        // Split each line by separator, align columns
        let rows:Vec<Vec<String>>=lines.iter().map(|l|l.split(sep).map(|s|s.to_string()).collect()).collect();
        let ncols=rows.iter().map(|r|r.len()).max().unwrap_or(0);
        let mut col_widths=alloc::vec![0usize;ncols];
        for row in &rows{for (i,cell) in row.iter().enumerate(){if i<ncols{col_widths[i]=col_widths[i].max(cell.len());}}}
        for row in &rows{
            for (i,cell) in row.iter().enumerate(){
                wstr(cell);
                if i+1<row.len(){ for _ in 0..(col_widths[i]+2).saturating_sub(cell.len()){write(STDOUT,b" ");} }
            }
            write(STDOUT,b"\n");
        }
    } else {
        // Multi-column layout
        let max_w=lines.iter().map(|l|l.len()).max().unwrap_or(1)+2;
        let cols=(width/max_w).max(1);
        let rows_count=(lines.len()+cols-1)/cols;
        for row in 0..rows_count{
            for col in 0..cols{
                let idx=if fillrows{row*cols+col}else{col*rows_count+row};
                if idx<lines.len(){
                    wstr(&lines[idx]);
                    if col+1<cols{for _ in 0..max_w.saturating_sub(lines[idx].len()){write(STDOUT,b" ");}}
                }
            }
            write(STDOUT,b"\n");
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
