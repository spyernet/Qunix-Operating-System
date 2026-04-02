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

#[repr(C)] struct Sysinfo{up:i64,ld:[u64;3],total:u64,free:u64,shared:u64,buf:u64,tswap:u64,fswap:u64,procs:u16,_p:[u8;22]}
#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a=args(argc,argv);
    let mut human=false;let mut unit=1024u64;let mut unit_n="Ki";let mut wide=false;
    let mut i=1;while i<a.len(){
        match a[i].as_str(){
            "-h"|"--human"=>human=true,"-g"=>{unit=1<<30;unit_n="G";}
            "-m"=>{unit=1<<20;unit_n="M";},"-k"|"--kilo"=>{unit=1024;unit_n="K";}
            "--mebi"=>{unit=1<<20;unit_n="Mi";},"--gibi"=>{unit=1<<30;unit_n="Gi";}
            "-w"|"--wide"=>wide=true, _=>{}
        }
        i+=1;
    }
    let mut si=Sysinfo{up:0,ld:[0;3],total:0,free:0,shared:0,buf:0,tswap:0,fswap:0,procs:0,_p:[0;22]};
    unsafe{syscall::syscall1(99,&mut si as *mut _ as u64)};
    let fmt=|n:u64|->String{
        if human{
            if n<1024{alloc::format!("{:6}B",n)}
            else if n<1<<20{alloc::format!("{:5.1}K",n as f64/1024.0)}
            else if n<1<<30{alloc::format!("{:5.1}M",n as f64/(1<<20)as f64)}
            else{alloc::format!("{:5.1}G",n as f64/(1<<30)as f64)}
        }else{alloc::format!("{:12}",(n+unit/2)/unit)}
    };
    let total=si.total;let free=si.free;let used=total.saturating_sub(free);
    let avail=free;let bufc=si.buf;
    wstr(&alloc::format!("{:>15}{:>12}{:>12}{:>12}{:>12}{:>12}\n","total","used","free","shared","buff/cache","available"));
    wstr("Mem:          ");
    wstr(&fmt(total));wstr(&fmt(used));wstr(&fmt(free));wstr(&fmt(si.shared));wstr(&fmt(bufc));wstr(&fmt(avail));wstr("\n");
    wstr("Swap:         ");
    wstr(&fmt(si.tswap));wstr(&fmt(si.tswap.saturating_sub(si.fswap)));wstr(&fmt(si.fswap));wstr("\n");
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
