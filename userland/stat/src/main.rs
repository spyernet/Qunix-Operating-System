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

#[repr(C)] struct Stat{dev:u64,ino:u64,nlink:u64,mode:u32,uid:u32,gid:u32,_p:u32,rdev:u64,size:i64,blksize:i64,blocks:i64,atime:i64,_an:i64,mtime:i64,_mn:i64,ctime:i64,_cn:i64,_u:[i64;3]}
#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = args(argc, argv);
    let mut fmt: Option<String>=None; let mut terse=false; let mut deref=false;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len() {
        match a[i].as_str() {
            "-c"|"--format" => { i+=1; fmt=a.get(i).cloned(); }
            "-t"|"--terse" => terse=true,
            "-L"|"--dereference" => deref=true,
            s if s.starts_with("--format=") => fmt=Some(s[9..].to_string()),
            s if s.starts_with("-c") => fmt=Some(s[2..].to_string()),
            _ => files.push(a[i].clone()),
        }
        i+=1;
    }
    let mut status=0;
    for f in &files {
        let mut st = Stat{dev:0,ino:0,nlink:0,mode:0,uid:0,gid:0,_p:0,rdev:0,size:0,blksize:0,blocks:0,atime:0,_an:0,mtime:0,_mn:0,ctime:0,_cn:0,_u:[0;3]};
        let mut p = f.clone(); p.push('\0');
        let r = unsafe { syscall::syscall2(4, p.as_ptr() as u64, &mut st as *mut _ as u64) };
        if r < 0 { werr(&alloc::format!("stat: {}: No such file\n",f)); status=1; continue; }
        let m = st.mode;
        let ft = match m&0xF000{0x8000=>"regular file",0x4000=>"directory",0xA000=>"symbolic link",0x2000=>"character device",0x6000=>"block device",0x1000=>"fifo",0xC000=>"socket",_=>"regular file"};
        let tc = match m&0xF000{0x8000=>'-',0x4000=>'d',0xA000=>'l',0x2000=>'c',0x6000=>'b',0x1000=>'p',0xC000=>'s',_=>'-'};
        let perm_s = {
            let b=|s:u32,bit:u32,c:char|if m>>s&bit!=0{c}else{'-'};
            alloc::format!("{}{}{}{}{}{}{}{}{}",b(6,4,'r'),b(6,2,'w'),b(6,1,'x'),b(3,4,'r'),b(3,2,'w'),b(3,1,'x'),b(0,4,'r'),b(0,2,'w'),b(0,1,'x'))
        };
        if let Some(ref f2) = fmt {
            let o = f2.replace("%n",f).replace("%s",&st.size.to_string())
                .replace("%b",&st.blocks.to_string()).replace("%B","512")
                .replace("%i",&st.ino.to_string()).replace("%h",&st.nlink.to_string())
                .replace("%u",&st.uid.to_string()).replace("%g",&st.gid.to_string())
                .replace("%a",&alloc::format!("{:o}",m&0o777))
                .replace("%A",&alloc::format!("{}{}",tc,perm_s))
                .replace("%F",ft).replace("%x",&st.atime.to_string())
                .replace("%y",&st.mtime.to_string()).replace("%z",&st.ctime.to_string())
                .replace("%d",&st.dev.to_string()).replace("%o",&st.blksize.to_string());
            wstr(&o); wstr("\n");
        } else if terse {
            wstr(&alloc::format!("{} {} {} {:04o} {} {} {} {} {} {} {} {} {} {}\n",
                f,st.size,st.blocks,m,st.uid,st.gid,st.rdev,st.ino,st.nlink,0,st.atime,st.mtime,st.ctime,st.blksize));
        } else {
            wstr(&alloc::format!("  File: {}\n  Size: {}\t\tBlocks: {}\t IO Block: {}\t{}\n",f,st.size,st.blocks,st.blksize,ft));
            wstr(&alloc::format!("Device: {:x}h\t\tInode: {}\t Links: {}\n",st.dev,st.ino,st.nlink));
            wstr(&alloc::format!("Access: ({:04o}/{}{})\t Uid: ( {:4}/    root) Gid: ( {:4}/    root)\n",m&0o7777,tc,perm_s,st.uid,st.gid));
            wstr(&alloc::format!("Access: {}\nModify: {}\nChange: {}\n Birth: -\n",st.atime,st.mtime,st.ctime));
        }
    }
    exit(status)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
