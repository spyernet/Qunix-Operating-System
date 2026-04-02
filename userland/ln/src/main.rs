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
    let mut symbolic=false; let mut force=false; let mut verbose=false;
    let mut no_dereference=false; let mut backup=false; let mut relative=false;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i < a.len() {
        match a[i].as_str() {
            "-s"|"--symbolic"       => symbolic=true,
            "-f"|"--force"          => force=true,
            "-v"|"--verbose"        => verbose=true,
            "-n"|"--no-dereference" => no_dereference=true,
            "-b"|"--backup"         => backup=true,
            "-r"|"--relative"       => relative=true,
            "--help"                => { w("Usage: ln [OPTION]... TARGET LINK_NAME\n"); exit(0); }
            "--version"             => { w("ln (Qunix) 1.0\n"); exit(0); }
            "--"                    => { i+=1; break; }
            s if s.starts_with('-') && !s.starts_with("--") => {
                for c in s[1..].chars() {
                    match c { 's'=>symbolic=true,'f'=>force=true,'v'=>verbose=true,
                              'n'=>no_dereference=true,'b'=>backup=true,'r'=>relative=true,_=>{} }
                }
            }
            _ => break,
        }
        i+=1;
    }
    while i < a.len() { files.push(a[i].clone()); i+=1; }
    if files.len() < 2 { e("ln: missing file operand\n"); exit(1); }
    let dst = files.last().unwrap().clone();
    let srcs = &files[..files.len()-1];
    // Is dst a directory?
    let dst_is_dir = { let mut p=dst.clone(); p.push('\0');
        let mut st=[0u64;22]; (unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)})==0
        && (st[2]>>32)&0xF000==0x4000 };
    let mut status = 0i32;
    for src in srcs {
        let link_path = if dst_is_dir {
            let base = src.rsplit('/').next().unwrap_or(src);
            alloc::format!("{}/{}", dst, base)
        } else { dst.clone() };
        if force {
            let mut p=link_path.clone(); p.push('\0');
            unsafe { syscall::syscall1(87, p.as_ptr() as u64) };
        }
        if backup {
            let bk = alloc::format!("{}~", link_path);
            let mut s=link_path.clone(); s.push('\0');
            let mut d=bk.clone(); d.push('\0');
            unsafe { syscall::syscall2(82, s.as_ptr() as u64, d.as_ptr() as u64) };
        }
        if verbose { w(&alloc::format!("'{}' -> '{}'\n", src, link_path)); }
        let mut s = src.clone(); s.push('\0');
        let mut d = link_path.clone(); d.push('\0');
        let r = if symbolic {
            unsafe { syscall::syscall2(88, s.as_ptr() as u64, d.as_ptr() as u64) }
        } else {
            unsafe { syscall::syscall2(86, s.as_ptr() as u64, d.as_ptr() as u64) }
        };
        if r < 0 {
            e(&alloc::format!("ln: failed to create {} link '{}' -> '{}': already exists\n",
                if symbolic {"symbolic"} else {"hard"}, link_path, src));
            status = 1;
        }
    }
    exit(status)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
