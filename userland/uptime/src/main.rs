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
pub extern "C" fn _start() -> ! {
    let mut ts=[0i64;2]; clock_gettime(1,&mut ts);
    let up=ts[0]as u64;
    let mut ts2=[0i64;2]; clock_gettime(0,&mut ts2);
    let epoch=ts2[0];
    let h=(epoch%86400)/3600; let m=(epoch%3600)/60; let s=epoch%60;
    let days=up/86400; let hrs=(up%86400)/3600; let mins=(up%3600)/60;
    wstr(&alloc::format!(" {:02}:{:02}:{:02} up ",h,m,s));
    if days>0{wstr(&alloc::format!("{} day{}, ",days,if days==1{""}else{"s"}));}
    wstr(&alloc::format!("{:2}:{:02},  1 user,  load average: 0.00, 0.00, 0.00\n",hrs,mins));
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
