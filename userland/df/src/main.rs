#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;


fn args_from_argv(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i);
        let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }).collect()
}
fn write_str(s: &str) { write(STDOUT, s.as_bytes()); }
fn write_err(s: &str) { write(STDERR, s.as_bytes()); }

#[repr(C)] struct StatFs{t:i64,bs:i64,bl:u64,bf:u64,ba:u64,fi:u64,ff:u64,fsid:[i32;2],nl:i64,fr:i64,fl:i64,sp:[i64;4]}
#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let args = args_from_argv(argc, argv);
    let mut human=false; let mut si=false; let mut inodes=false; let mut type_only=false;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<args.len(){
        match args[i].as_str(){
            "-h"|"--human-readable"=>human=true,"-H"|"--si"=>{human=true;si=true;}
            "-i"|"--inodes"=>inodes=true,"-T"|"--print-type"=>type_only=true,
            "--total"=>{}
            s if s.starts_with('-')&&!s.starts_with("--")=>{for c in s[1..].chars(){match c{'h'=>human=true,'H'=>{human=true;si=true;},'i'=>inodes=true,'T'=>type_only=true,_=>{}}}}
            _=>files.push(args[i].clone()),
        }
        i+=1;
    }
    if files.is_empty(){files.push("/".to_string());}
    if inodes{write_str("Filesystem        Inodes  IUsed   IFree IUse% Mounted on\n");}
    else if type_only{write_str("Filesystem     Type      1K-blocks     Used Available Use% Mounted on\n");}
    else if human{write_str("Filesystem       Size  Used Avail Use% Mounted on\n");}
    else{write_str("Filesystem     1K-blocks     Used Available Use% Mounted on\n");}
    for f in &files {
        let mut sf=StatFs{t:0,bs:0,bl:0,bf:0,ba:0,fi:0,ff:0,fsid:[0;2],nl:0,fr:0,fl:0,sp:[0;4]};
        let mut p=f.clone();p.push('\0');
        unsafe{syscall::syscall2(137,p.as_ptr() as u64,&mut sf as *mut _ as u64)};
        let total=sf.bl*sf.bs as u64; let free=sf.bf*sf.bs as u64; let used=total.saturating_sub(free);
        let pct=if total>0{used*100/total}else{0};
        let fmt_sz=|n:u64|->String{if !human{alloc::format!("{:12}",n/1024)}else{
            if si{if n<1000{alloc::format!("{:4}B",n)}else if n<1000000{alloc::format!("{:.1}K",n as f64/1000.0)}else if n<1000000000{alloc::format!("{:.1}M",n as f64/1000000.0)}else{alloc::format!("{:.1}G",n as f64/1000000000.0)}}
            else{if n<1024{alloc::format!("{:4}B",n)}else if n<1048576{alloc::format!("{:.1}K",n as f64/1024.0)}else if n<1073741824{alloc::format!("{:.1}M",n as f64/1048576.0)}else{alloc::format!("{:.1}G",n as f64/1073741824.0)}}
        }};
        let fs_type=match sf.t{0xEF53=>"ext4",0x01021994=>"tmpfs",0x65735546=>"fusefs",0x9123683e=>"btrfs",_=>"unknown"};
        if inodes{write_str(&alloc::format!("{:<18} {:>8} {:>7} {:>7} {:>4}% {}\n",f,sf.fi,sf.fi.saturating_sub(sf.ff),sf.ff,if sf.fi>0{(sf.fi-sf.ff)*100/sf.fi}else{0},f));}
        else if type_only{write_str(&alloc::format!("{:<18} {:<8} {} {} {} {:>4}% {}\n",f,fs_type,fmt_sz(total),fmt_sz(used),fmt_sz(free),pct,f));}
        else{write_str(&alloc::format!("{:<18} {} {} {} {:>4}% {}\n",f,fmt_sz(total),fmt_sz(used),fmt_sz(free),pct,f));}
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
