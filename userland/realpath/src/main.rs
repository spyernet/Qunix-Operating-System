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
    let a=args(argc,argv); let mut quiet=false;let mut zero=false;let mut i=1;
    while i<a.len(){match a[i].as_str(){"-q"|"--quiet"=>quiet=true,"-z"|"--zero"=>zero=true,"--"=>{i+=1;break;},s if s.starts_with('-')=>{},_=>break,}i+=1;}
    let sep=if zero{"\0"}else{"\n"};
    let mut status=0i32;
    for f in &a[i..] {
        let abs=if f.starts_with('/'){ f.clone() }else{
            let mut cwd=[0u8;4096];let n=getcwd(&mut cwd);
            let l=cstr_len(&cwd);
            alloc::format!("{}/{}",String::from_utf8_lossy(&cwd[..l]),f)
        };
        // Canonicalize ./ ../
        let mut parts: Vec<&str>=Vec::new();
        for c in abs.split('/'){match c{""=>{},"."=>{},".."=>{parts.pop();}x=>parts.push(x)}}
        let canon=if parts.is_empty(){"/".to_string()}else{alloc::format!("/{}",parts.join("/"))};
        // Check existence
        let mut p=canon.clone();p.push('\0');
        let mut st=[0u64;22];
        if unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)}<0{
            if !quiet{werr(&alloc::format!("realpath: {}: No such file or directory\n",f));}
            status=1;
        } else {
            wstr(&canon); wstr(sep);
        }
    }
    exit(status)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
