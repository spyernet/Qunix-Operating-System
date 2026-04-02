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
    let mut show_u=false; let mut show_g=false; let mut show_G=false;
    let mut name_only=false; let mut real=false; let mut zero=false;
    let mut i=1;
    while i < a.len() {
        match a[i].as_str() {
            "-u"|"--user"    => show_u=true,
            "-g"|"--group"   => show_g=true,
            "-G"|"--groups"  => show_G=true,
            "-n"|"--name"    => name_only=true,
            "-r"|"--real"    => real=true,
            "-z"|"--zero"    => zero=true,
            "--help"         => { w("Usage: id [OPTION]... [USER]\nPrint user and group information.\n"); exit(0); }
            "--version"      => { w("id (Qunix) 1.0\n"); exit(0); }
            _                => {}
        }
        i+=1;
    }
    let uid  = unsafe { syscall::syscall0(SYS_GETUID)  } as u32;
    let gid  = unsafe { syscall::syscall0(SYS_GETGID)  } as u32;
    let euid = unsafe { syscall::syscall0(SYS_GETEUID) } as u32;
    let egid = unsafe { syscall::syscall0(SYS_GETEGID) } as u32;
    let name_for = |id: u32| -> String {
        let passwd = rdfile("/etc/passwd");
        let text = String::from_utf8_lossy(&passwd);
        for line in text.lines() {
            let parts: Vec<&str>=line.split(':').collect();
            if parts.len()>=3 { if let Ok(u)=parts[2].parse::<u32>() { if u==id { return parts[0].to_string(); } } }
        }
        id.to_string()
    };
    let sep: &[u8] = if zero { b"\0" } else { b"\n" };
    if show_u {
        let id = if real { uid } else { euid };
        let out = if name_only { name_for(id) } else { id.to_string() };
        w(&out);
        write(STDOUT, sep); exit(0);
    }
    if show_g {
        let id = if real { gid } else { egid };
        let out = if name_only { name_for(id) } else { id.to_string() };
        w(&out);
        write(STDOUT, sep); exit(0);
    }
    if show_G {
        w(&gid.to_string()); write(STDOUT, sep); exit(0);
    }
    let uname = name_for(uid); let gname = name_for(gid);
    w(&alloc::format!("uid={}({}) gid={}({}) groups={}({})", uid, uname, gid, gname, gid, gname));
    if euid != uid { w(&alloc::format!(" euid={}({})", euid, name_for(euid))); }
    if egid != gid { w(&alloc::format!(" egid={}({})", egid, name_for(egid))); }
    write(STDOUT, sep);
    exit(0)
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
