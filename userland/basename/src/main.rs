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
    let mut zero=false; let mut multiple=false; let mut suffix=String::new();
    let mut paths: Vec<String>=Vec::new(); let mut i=1;
    while i < a.len() {
        match a[i].as_str() {
            "-z"|"--zero"          => zero=true,
            "-a"|"--multiple"      => multiple=true,
            "-s"|"--suffix"        => { i+=1; suffix=a.get(i).cloned().unwrap_or_default(); }
            "--help"               => { w("Usage: basename NAME [SUFFIX]\n"); exit(0); }
            "--version"            => { w("basename (Qunix) 1.0\n"); exit(0); }
            "--"                   => { i+=1; break; }
            s if s.starts_with("-s") => suffix=s[2..].to_string(),
            _                      => break,
        }
        i+=1;
    }
    while i < a.len() { paths.push(a[i].clone()); i+=1; }
    if paths.is_empty() { e("basename: missing operand\n"); exit(1); }
    // If not multiple mode and exactly 2 args remain, second is suffix
    if !multiple && paths.len() == 2 && suffix.is_empty() {
        suffix = paths.pop().unwrap();
    }
    let sep: &[u8] = if zero { b"\0" } else { b"\n" };
    for path in &paths {
        let base = path.trim_end_matches('/');
        let base = if let Some(s) = base.rfind('/') { &base[s+1..] } else { base };
        let base = if !suffix.is_empty() { base.strip_suffix(suffix.as_str()).unwrap_or(base) } else { base };
        w(base); write(STDOUT, sep);
    }
    exit(0)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
