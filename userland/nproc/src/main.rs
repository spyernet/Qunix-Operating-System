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
    // Read from /sys/devices/system/cpu/online or fallback
    let fd = open(b"/sys/devices/system/cpu/online\0", O_RDONLY, 0);
    let count = if fd >= 0 {
        let mut buf=[0u8;64]; let n=read(fd as i32,&mut buf); close(fd as i32);
        if n>0 { parse_cpu_range(&String::from_utf8_lossy(&buf[..n as usize])) } else { 1 }
    } else {
        unsafe { syscall::syscall3(228,0,0,0) as usize }.max(1)
    };
    let all = a.iter().any(|s| s=="--all");
    wstr(&alloc::format!("{}\n", count));
    exit(0)
}
fn parse_cpu_range(s: &str) -> usize {
    let s=s.trim(); let mut total=0usize;
    for part in s.split(','){
        if let Some(dash)=part.find('-'){
            let a:usize=part[..dash].parse().unwrap_or(0);
            let b:usize=part[dash+1..].parse().unwrap_or(a);
            total+=b-a+1;
        } else { total+=1; }
    }
    total.max(1)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
