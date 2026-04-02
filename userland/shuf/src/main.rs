#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;


fn av(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i); let mut n=0; while *p.add(n)!=0{n+=1;}
        String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).to_string()
    }).collect()
}
fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> String {
    let mut d=alloc::vec![0u8;1<<20]; let mut t=0;
    loop{if t>=d.len(){d.resize(d.len()*2,0);} let n=read(fd,&mut d[t..]); if n<=0{break;} t+=n as usize;}
    String::from_utf8_lossy(&d[..t]).to_string()
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = av(argc, argv);
    let mut count: Option<usize>=None; let mut zero=false; let mut repeat=false;
    let mut range: Option<(i64,i64)>=None; let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-n"|"--head-count"=>{ i+=1; count=a.get(i).and_then(|s|s.parse().ok()); }
            "-z"|"--zero-terminated"=>zero=true,
            "-r"|"--repeat"=>repeat=true,
            "-i"|"--input-range"=>{ i+=1; if let Some(r)=a.get(i){ if let Some(d)=r.find('-'){ let lo=r[..d].parse().unwrap_or(0); let hi=r[d+1..].parse().unwrap_or(0); range=Some((lo,hi)); } } }
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    let sep:&[u8]=if zero{b"\0"}else{b"\n"};
    let mut lines:Vec<String>=if let Some((lo,hi))=range {
        (lo..=hi).map(|n|n.to_string()).collect()
    } else {
        let mut v=Vec::new();
        let process=|fd:i32,v:&mut Vec<String>|{let s=rdall(fd);for l in s.split('\n'){if !l.is_empty(){v.push(l.to_string());}}};
        if files.is_empty(){process(STDIN,&mut v);}
        else{for f in &files{if f=="-"{process(STDIN,&mut v);}else{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{continue;}process(fd as i32,&mut v);close(fd as i32);}}}
        v
    };
    // Fisher-Yates shuffle with xorshift RNG
    let mut ts=[0i64;2]; clock_gettime(1,&mut ts);
    let mut rng=(ts[1] as u64)^((ts[0] as u64).wrapping_mul(6364136223846793005));
    let xshift=|r:&mut u64|{*r^=*r<<13;*r^=*r>>7;*r^=*r<<17;*r};
    for i in (1..lines.len()).rev(){
        let j=(xshift(&mut rng) as usize)%(i+1);
        lines.swap(i,j);
    }
    let limit=count.unwrap_or(lines.len());
    for l in lines.iter().take(limit){write(STDOUT,l.as_bytes());write(STDOUT,sep);}
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
