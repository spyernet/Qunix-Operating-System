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

#[repr(C)] struct Utsname { s:[u8;65],n:[u8;65],r:[u8;65],v:[u8;65],m:[u8;65],d:[u8;65] }
#[no_mangle] #[link_section=".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = parse_argv(argc, argv);
    let mut show_s=false; let mut show_n=false; let mut show_r=false;
    let mut show_v=false; let mut show_m=false; let mut show_p=false;
    let mut show_i=false; let mut show_o=false; let mut all=false;
    let mut i=1;
    while i < a.len() {
        match a[i].as_str() {
            "-a"|"--all"              => all=true,
            "-s"|"--kernel-name"      => show_s=true,
            "-n"|"--nodename"         => show_n=true,
            "-r"|"--kernel-release"   => show_r=true,
            "-v"|"--kernel-version"   => show_v=true,
            "-m"|"--machine"          => show_m=true,
            "-p"|"--processor"        => show_p=true,
            "-i"|"--hardware-platform"=> show_i=true,
            "-o"|"--operating-system" => show_o=true,
            "--help"                  => { w("Usage: uname [OPTION]...\n"); exit(0); }
            "--version"               => { w("uname (Qunix) 1.0\n"); exit(0); }
            s if s.starts_with('-') && !s.starts_with("--") => {
                for c in s[1..].chars() {
                    match c { 'a'=>all=true,'s'=>show_s=true,'n'=>show_n=true,
                              'r'=>show_r=true,'v'=>show_v=true,'m'=>show_m=true,
                              'p'=>show_p=true,'i'=>show_i=true,'o'=>show_o=true, _=>{} }
                }
            }
            _ => {}
        }
        i+=1;
    }
    if !all && !show_s && !show_n && !show_r && !show_v && !show_m && !show_p && !show_i && !show_o {
        show_s = true;
    }
    let mut uts = Utsname{s:[0;65],n:[0;65],r:[0;65],v:[0;65],m:[0;65],d:[0;65]};
    unsafe { syscall::syscall1(SYS_UNAME, &mut uts as *mut _ as u64) };
    let slen=|b:&[u8]|b.iter().position(|&x|x==0).unwrap_or(b.len());
    let ss=|b:&[u8]|String::from_utf8_lossy(&b[..b.iter().position(|&x|x==0).unwrap_or(b.len())]).to_string();
    let mut parts: Vec<String>=Vec::new();
    if all||show_s { parts.push(ss(&uts.s)); }
    if all||show_n { parts.push(ss(&uts.n)); }
    if all||show_r { parts.push(ss(&uts.r)); }
    if all||show_v { parts.push(ss(&uts.v)); }
    if all||show_m { parts.push(ss(&uts.m)); }
    if all||show_p { parts.push("x86_64".to_string()); }
    if all||show_i { parts.push("x86_64".to_string()); }
    if all||show_o { parts.push("Qunix".to_string()); }
    w(&parts.join(" ")); w("\n");
    exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
