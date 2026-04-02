#![no_std]
#![no_main]
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
    let silent = a.iter().any(|s| s=="-s"||s=="--silent"||s=="--quiet");
    if a.iter().any(|s| s=="--help") {
        w("Usage: tty [-s]\nPrint path of terminal connected to standard input.\n"); exit(0);
    }
    // Check if stdin is a tty via TIOCGWINSZ ioctl (0x5413)
    let mut ws = [0u16; 4];
    let r = unsafe { syscall::syscall3(SYS_IOCTL, STDIN as u64, 0x5413, ws.as_mut_ptr() as u64) };
    if r < 0 {
        if !silent { w("not a tty\n"); }
        exit(1);
    }
    if !silent {
        // Read symlink /proc/self/fd/0
        let mut buf = [0u8; 256];
        let n = unsafe { syscall::syscall3(SYS_READLINK,
            b"/proc/self/fd/0\0".as_ptr() as u64,
            buf.as_mut_ptr() as u64, 256) };
        if n > 0 {
            w(&String::from_utf8_lossy(&buf[..n as usize]));
            w("\n");
        } else {
            w("/dev/tty\n");
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
