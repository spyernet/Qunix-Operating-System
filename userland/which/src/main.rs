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
    let a=args(argc,argv);
    let mut all=false; let mut i=1;
    while i<a.len(){match a[i].as_str(){"-a"|"--all"=>all=true,"--"=>{i+=1;break;},_=>break,}i+=1;}
    let path_var=rdfile("/proc/self/environ");
    let path=path_var.split('\0').find(|e|e.starts_with("PATH="))
        .map(|e|e[5..].to_string()).unwrap_or_else(||"/bin:/sbin:/usr/bin:/usr/sbin:/usr/local/bin".to_string());
    let mut status=0i32;
    for cmd in a[i..].iter().filter(|s|!s.starts_with('-')) {
        let mut found=false;
        for dir in path.split(':') {
            let full=alloc::format!("{}/{}\0",dir,cmd);
            let mut st=[0u8;176];
            if (unsafe{syscall::syscall2(4,full.as_ptr() as u64,st.as_mut_ptr() as u64)})==0 {
                let mode=u32::from_le_bytes(st[24..28].try_into().unwrap_or([0;4]));
                if mode&0o111!=0{
                    wstr(&full[..full.len()-1]); wstr("\n"); found=true;
                    if !all{break;}
                }
            }
        }
        if !found{werr(&alloc::format!("{}: not found\n",cmd));status=1;}
    }
    exit(status)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
