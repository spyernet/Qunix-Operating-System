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
    let mut human=false; let mut summary=false; let mut max_depth: Option<usize>=None;
    let mut bytes=false; let mut count_links=false; let mut total=false; let mut si=false;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<args.len(){
        match args[i].as_str(){
            "-h"|"--human-readable"=>human=true,"-H"|"--si"=>{human=true;si=true;}
            "-s"|"--summarize"=>summary=true,"-b"|"--bytes"=>bytes=true,
            "-l"|"--count-links"=>count_links=true,"-c"|"--total"=>total=true,
            "-d"|"--max-depth"=>{i+=1;max_depth=args.get(i).and_then(|s|s.parse().ok());}
            s if s.starts_with("--max-depth=")=>{max_depth=s[12..].parse().ok();}
            s if s.starts_with("--")||s=="-"=>{files.push(args[i].clone());}
            s if s.starts_with('-')=>{
                let mut j=1;
                while j<s.len(){
                    match s.as_bytes()[j]{b'h'=>human=true,b'H'=>{human=true;si=true;},b's'=>summary=true,b'b'=>bytes=true,b'l'=>count_links=true,b'c'=>total=true,b'd'=>{j+=1;max_depth=s[j..].parse().ok();break;},_=>{}}
                    j+=1;
                }
            }
            _=>files.push(args[i].clone()),
        }
        i+=1;
    }
    if files.is_empty(){files.push(".".to_string());}
    let fmt=|n:u64|->String{
        if bytes{return alloc::format!("{:8}",n);}
        let kb=(n+511)/512; // 512-byte blocks
        if !human{return alloc::format!("{:8}",kb);}
        let n2=n; // bytes
        if n2<1024{alloc::format!("{:4}B",n2)}else if n2<1048576{alloc::format!("{:.1}K",n2 as f64/1024.0)}else if n2<1073741824{alloc::format!("{:.1}M",n2 as f64/1048576.0)}else{alloc::format!("{:.1}G",n2 as f64/1073741824.0)}
    };
    let mut grand_total=0u64;
    for f in &files {
        let sz=du_recurse(f,0,max_depth,summary,&fmt);
        if summary{write_str(&alloc::format!("{}\t{}\n",fmt(sz),f));}
        grand_total+=sz;
    }
    if total{write_str(&alloc::format!("{}\ttotal\n",fmt(grand_total)));}
    exit(0)
}

fn du_recurse(path: &str, depth: usize, max_depth: Option<usize>, summary: bool, fmt: &impl Fn(u64)->String) -> u64 {
    let mut p=path.to_string();p.push('\0');
    let mut st=[0u64;22];
    if unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)}<0{return 0;}
    let size=st[8]*512; // st_blocks * 512
    let mode=(st[2]>>32) as u32;
    let is_dir=mode&0xF000==0x4000;
    if !is_dir{return st[9] as u64;} // st_size for files
    let mut total=size;
    let fd=open(p.as_bytes(),0o200000,0);
    if fd>=0{
        let mut buf=alloc::vec![0u8;32768];let mut entries=Vec::new();
        loop{let n=getdents64(fd as i32,&mut buf);if n<=0{break;}
            let mut off=0;while off<n as usize{
                let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2]))as usize;
                let name=&buf[off+19..];let nlen=name.iter().position(|&b|b==0).unwrap_or(0);
                let name_s=String::from_utf8_lossy(&name[..nlen]).to_string();
                if name_s!="."&&name_s!=".."{entries.push(name_s);}
                if reclen==0{break;}off+=reclen;
            }
        }
        close(fd as i32);
        for e in entries{
            let sub=alloc::format!("{}/{}",path,e);
            let sub_sz=du_recurse(&sub,depth+1,max_depth,summary,fmt);
            total+=sub_sz;
        }
    }
    if !summary{
        if max_depth.map_or(true,|m| depth<=m){write_str(&alloc::format!("{}\t{}\n",fmt(total),path));}
    }
    total
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
