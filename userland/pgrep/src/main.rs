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
    let is_kill = a.get(0).map(|s| s.ends_with("pkill")).unwrap_or(false);
    let mut pattern=String::new(); let mut list=false; let mut full=false; let mut signal=15i32;
    let mut i=1; while i<a.len(){
        match a[i].as_str(){
            "-l"|"--list-name"=>list=true, "-f"|"--full"=>full=true,
            "-signal"|"-s"=>{ i+=1; signal=a.get(i).map(|s|s.parse().unwrap_or(15)).unwrap_or(15); }
            s if s.starts_with('-')&&s[1..].chars().all(|c|c.is_ascii_digit())=>signal=s[1..].parse().unwrap_or(15),
            _=>if pattern.is_empty(){pattern=a[i].clone()}
        }
        i+=1;
    }
    let mut found=false;
    // Scan /proc
    let fd=open(b"/proc\0",0o200000,0); if fd>=0 {
        let mut buf=alloc::vec![0u8;32768]; let mut pids=Vec::new();
        loop{let n=getdents64(fd as i32,&mut buf);if n<=0{break;}
            let mut off=0; while off<n as usize{
                let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2]))as usize;
                let name=&buf[off+19..];let nlen=name.iter().position(|&b|b==0).unwrap_or(0);
                let name_s=String::from_utf8_lossy(&name[..nlen]).to_string();
                if name_s.chars().all(|c|c.is_ascii_digit()){ if let Ok(pid)=name_s.parse::<i32>(){pids.push(pid);} }
                if reclen==0{break;} off+=reclen;
            }
        }
        close(fd as i32);
        for pid in pids {
            let cmd_path=alloc::format!("/proc/{}/cmdline\0",pid);
            let cfd=open(cmd_path.as_bytes(),O_RDONLY,0); if cfd<0{continue;}
            let mut cbuf=[0u8;1024]; let n=read(cfd as i32,&mut cbuf); close(cfd as i32);
            if n<=0{continue;}
            let cmdline=String::from_utf8_lossy(&cbuf[..n as usize]).replace('\0'," ");
            let name=cmdline.split_whitespace().next().unwrap_or("").rsplit('/').next().unwrap_or("");
            let match_str=if full{&cmdline[..]}else{name};
            if pattern.is_empty()||match_str.contains(pattern.as_str()){
                found=true;
                if is_kill{kill(pid,signal);}
                else{
                    if list{wstr(&alloc::format!("{} {}\n",pid,name));}
                    else{wstr(&alloc::format!("{}\n",pid));}
                }
            }
        }
    }
    exit(if found{0}else{1})
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
