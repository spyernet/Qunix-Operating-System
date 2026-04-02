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
    let a = args(argc, argv);
    let mut set_name: Option<String>=None; let mut i=1;
    while i<a.len(){match a[i].as_str(){"-s"|"--short"=>{},"--"=>{i+=1;break;},s if !s.starts_with('-')=>{set_name=Some(s.to_string());}  ,_=>{}}i+=1;}
    if let Some(ref n)=set_name {
        let mut h=n.clone();h.push('\0');
        unsafe{syscall::syscall2(170,h.as_ptr() as u64,h.len() as u64-1)};
    } else {
        let s=rdfile("/etc/hostname");
        let h=s.trim();
        if h.is_empty(){wstr("qunix");}else{wstr(h);}
        wstr("\n");
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
