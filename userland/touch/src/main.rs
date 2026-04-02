#![no_std]
#![no_main]
#![allow(unused_variables, unused_assignments, unused_mut, dead_code)]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;


fn parse_argv(argc: u64, ap: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *ap.add(i); let mut n=0; while *p.add(n)!=0{n+=1;}
        String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).into_owned()
    }).collect()
}
fn w(s: &str) { write(STDOUT, s.as_bytes()); }
fn e(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> alloc::vec::Vec<u8> {
    let mut d=alloc::vec![0u8;1<<20]; let mut t=0;
    loop { if t>=d.len(){d.resize(d.len()*2,0);} let n=read(fd,&mut d[t..]); if n<=0{break;} t+=n as usize; }
    d.truncate(t); d
}
fn rdfile(path: &str) -> alloc::vec::Vec<u8> {
    let mut p=path.to_string(); p.push('\0');
    let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{return alloc::vec![];}
    let d=rdall(fd as i32); close(fd as i32); d
}
fn cstr(p: *const u8) -> String {
    unsafe { let mut n=0; while *p.add(n)!=0{n+=1;}
    String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).into_owned() }
}

#[no_mangle] #[link_section=".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = parse_argv(argc, argv);
    let mut no_create=false; let mut access_only=false; let mut mod_only=false;
    let mut ref_file: Option<String>=None; let mut date_str: Option<String>=None;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i < a.len() {
        match a[i].as_str() {
            "-c"|"--no-create"   => no_create=true,
            "-a"                 => access_only=true,
            "-m"                 => mod_only=true,
            "-r"|"--reference"   => { i+=1; ref_file=a.get(i).cloned(); }
            "-d"|"--date"        => { i+=1; date_str=a.get(i).cloned(); }
            "-t"                 => { i+=1; date_str=a.get(i).cloned(); }
            "--help"             => { w("Usage: touch [OPTION]... FILE...\n"); exit(0); }
            "--version"          => { w("touch (Qunix) 1.0\n"); exit(0); }
            "--"                 => { i+=1; break; }
            s if s.starts_with('-') && s.len()>1 && !s.starts_with("--") => {
                for c in s[1..].chars() {
                    match c { 'c'=>no_create=true,'a'=>access_only=true,'m'=>mod_only=true,_=>{} }
                }
            }
            _ => break,
        }
        i+=1;
    }
    while i < a.len() { files.push(a[i].clone()); i+=1; }
    if files.is_empty() { e("touch: missing file operand\n"); exit(1); }
    let mut status = 0i32;
    for f in &files {
        let mut p = f.clone(); p.push('\0');
        // Check existence
        let mut st = [0u64; 22];
        let exists = (unsafe { syscall::syscall2(4, p.as_ptr() as u64, st.as_mut_ptr() as u64) }) == 0;
        if !exists {
            if no_create { continue; }
            let fd = open(p.as_bytes(), O_CREAT|O_WRONLY, 0o666);
            if fd < 0 {
                e(&alloc::format!("touch: cannot touch '{}': Permission denied\n", f));
                status = 1; continue;
            }
            close(fd as i32);
        }
        // Update timestamps via utimensat(AT_FDCWD=-100, path, NULL=current time, 0)
        unsafe { syscall::syscall4(280, (-100i64) as u64, p.as_ptr() as u64, 0u64, 0u64) };
    }
    exit(status)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
