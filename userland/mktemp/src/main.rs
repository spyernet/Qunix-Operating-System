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
    let mut is_dir=false; let mut tmpdir="/tmp".to_string();
    let mut template="tmp.XXXXXXXXXX".to_string(); let mut i=1;
    while i < a.len() {
        match a[i].as_str() {
            "-d"|"--directory" => is_dir=true,
            "-p" => { i+=1; tmpdir=a.get(i).cloned().unwrap_or("/tmp".to_string()); }
            s if s.starts_with("--tmpdir=") => tmpdir=s[9..].to_string(),
            s if !s.starts_with('-') => template=s.to_string(),
            _ => {}
        }
        i+=1;
    }
    let mut ts=[0i64;2]; clock_gettime(1,&mut ts);
    let mut rng = (ts[1] as u64).wrapping_add((ts[0] as u64).wrapping_mul(0x9e3779b97f4a7c15));
    let chars: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let xcount = template.chars().rev().take_while(|&c| c=='X').count().max(6);
    let base = &template[..template.len()-xcount];
    let rand_part: String = (0..xcount).map(|_| {
        rng ^= rng<<13; rng ^= rng>>7; rng ^= rng<<17;
        chars[(rng as usize) % chars.len()] as char
    }).collect();
    let name = alloc::format!("{}/{}{}", tmpdir, base, rand_part);
    let mut p = name.clone(); p.push('\0');
    let ok = if is_dir {
        (unsafe { syscall::syscall2(83, p.as_ptr() as u64, 0o700u64) }) >= 0
    } else {
        let fd = open(p.as_bytes(), O_CREAT|O_EXCL|O_RDWR, 0o600);
        if fd >= 0 { close(fd as i32); true } else { false }
    };
    if ok { wstr(&name); wstr("\n"); exit(0); }
    else { werr("mktemp: failed to create temp\n"); exit(1); }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
