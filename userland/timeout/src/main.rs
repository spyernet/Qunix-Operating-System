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
    let mut kill_sig=15i32; let mut duration_s=0f64; let mut preserve_status=false; let mut foreground=false;
    let mut cmd_start=1usize;
    let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-s"|"--signal"=>{ i+=1; kill_sig=a.get(i).map(|s|sig_num(s)).unwrap_or(15); }
            "-k"|"--kill-after"=>{ i+=1; } // simplified: ignore kill-after
            "--preserve-status"=>preserve_status=true,
            "--foreground"=>foreground=true,
            "--"=>{cmd_start=i+1;break;}
            s if s.starts_with('-')=>{},
            _=>{duration_s=parse_dur(a[i].as_str());cmd_start=i+1;break;}
        }
        i+=1;
    }
    if cmd_start>=a.len(){exit(125);}
    let pid=fork();
    if pid==0{
        let args=&a[cmd_start..];
        let argv_strs:Vec<String>=args.iter().map(|s|{let mut x=s.clone();x.push('\0');x}).collect();
        let argv:Vec<*const u8>=argv_strs.iter().map(|s|s.as_ptr() as *const u8).chain(core::iter::once(core::ptr::null())).collect();
        let envp:[*const u8;1]=[core::ptr::null()];
        let mut cmd=a[cmd_start].clone();cmd.push('\0');
        execve(cmd.as_bytes(),&argv,&envp); exit(127);
    }
    if pid<=0{exit(125);}
    // Wait with timeout
    let deadline_ms=(duration_s*1000.0) as u64;
    let start_ms=unsafe{syscall::syscall0(228)} as u64;
    let mut status=0i32;
    loop{
        let r=waitpid(pid as i32,&mut status,1); // WNOHANG
        if r!=0{
            let exit_code=if status&0x7F==0{(status>>8)&0xFF}else{128+(status&0x7F)};
            exit(if preserve_status{exit_code}else{exit_code});
        }
        let now=unsafe{syscall::syscall0(228)} as u64;
        if deadline_ms>0&&now.wrapping_sub(start_ms)>=deadline_ms{
            kill(pid as i32,kill_sig); nanosleep_ms(100);
            kill(pid as i32,9);
            exit(124);
        }
        nanosleep_ms(10);
    }
}
fn parse_dur(s: &str) -> f64 {
    let last=s.chars().last().unwrap_or('s');
    let n=if last.is_alphabetic(){s[..s.len()-1].parse().unwrap_or(0.0)}else{s.parse().unwrap_or(0.0)};
    match last{'m'=>n*60.0,'h'=>n*3600.0,'d'=>n*86400.0,_=>n}
}
fn sig_num(s: &str) -> i32 { s.parse().unwrap_or(match s.to_uppercase().as_str(){"KILL"=>9,"TERM"=>15,"HUP"=>1,"INT"=>2,"QUIT"=>3,_=>15}) }

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
