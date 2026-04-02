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
    let mut all=false;let mut full=false;let mut users=false;let mut i=1;
    while i<a.len(){
        let arg=&a[i];
        if arg=="aux"||arg=="-aux"{all=true;users=true;full=true;}
        else if arg.contains('a'){all=true;} else if arg.contains('u'){users=true;} else if arg.contains('f'){full=true;}
        else if arg=="-e"||arg=="-A"{all=true;}else if arg=="-f"{full=true;}
        i+=1;
    }
    if users{wstr("USER         PID %CPU %MEM    VSZ   RSS TTY      STAT START   TIME COMMAND\n");}
    else if full{wstr("UID          PID    PPID  C STIME TTY          TIME CMD\n");}
    else{wstr("  PID TTY          TIME CMD\n");}
    // Read /proc
    let fd=open(b"/proc\0",0o200000,0);
    let mut pids=Vec::new();
    if fd>=0{
        let mut buf=alloc::vec![0u8;32768];
        loop{let n=getdents64(fd as i32,&mut buf);if n<=0{break;}
            let mut off=0;while off<n as usize{
                let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2]))as usize;
                let name=&buf[off+19..];let nlen=name.iter().position(|&b|b==0).unwrap_or(0);
                let s=String::from_utf8_lossy(&name[..nlen]).to_string();
                if s.chars().all(|c|c.is_ascii_digit()){if let Ok(n)=s.parse::<u32>(){pids.push(n);}}
                if reclen==0{break;}off+=reclen;
            }
        }
        close(fd as i32);
    } else {
        pids.push(unsafe{syscall::syscall0(39)}as u32);
    }
    pids.sort();
    let my_uid=unsafe{syscall::syscall0(102)}as u32;
    for pid in &pids{
        let mut name=alloc::format!("[{}]",pid);let mut ppid=0u32;let mut state='S';
        let mut uid=0u32;let mut vsz=0u64;let mut rss=0u64;
        let s=rdfile(&alloc::format!("/proc/{}/status",pid));
        for line in s.lines(){
            if line.starts_with("Name:"){name=line[5..].trim().to_string();}
            else if line.starts_with("PPid:"){ppid=line[5..].trim().parse().unwrap_or(0);}
            else if line.starts_with("State:"){state=line[6..].trim().chars().next().unwrap_or('S');}
            else if line.starts_with("Uid:"){uid=line[4..].trim().split_whitespace().next().and_then(|s|s.parse().ok()).unwrap_or(0);}
            else if line.starts_with("VmRSS:"){rss=line[6..].trim().split_whitespace().next().and_then(|s|s.parse().ok()).unwrap_or(0)*1024;}
            else if line.starts_with("VmSize:"){vsz=line[7..].trim().split_whitespace().next().and_then(|s|s.parse().ok()).unwrap_or(0)*1024;}
        }
        // cmdline
        let cl=rdfile(&alloc::format!("/proc/{}/cmdline",pid));
        if !cl.is_empty(){name=cl.replace('\0'," ").trim().to_string();}
        if !all&&uid!=my_uid{continue;}
        let stat_c=match state{'R'=>"R",'S'=>"S",'D'=>"D",'Z'=>"Z",'T'=>"T",_=>"S"};
        let uname=if uid==0{"root"}else{"user"};
        if users{wstr(&alloc::format!("{:<12} {:>5}  0.0  0.0 {:>6} {:>5} ?        {:<4} 00:00   0:00 {}\n",uname,pid,vsz/1024,rss/1024,stat_c,name));}
        else if full{wstr(&alloc::format!("{:<12} {:>6} {:>6}  0 00:00 ?            0:00 {}\n",uname,pid,ppid,name));}
        else{wstr(&alloc::format!("{:>5} ?            0:00 {}\n",pid,name));}
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
