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
    let mut width=80usize; let mut break_spaces=false; let mut break_bytes=false;
    let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){
        match a[i].as_str(){
            "-w"|"--width"=>{ i+=1; width=a.get(i).and_then(|s|s.parse().ok()).unwrap_or(80); }
            "-s"|"--spaces"=>break_spaces=true,
            "-b"|"--bytes"=>break_bytes=true,
            s if s.starts_with("-w")=>width=s[2..].parse().unwrap_or(80),
            _=>files.push(a[i].clone()),
        }
        i+=1;
    }
    let process=|fd:i32|{
        let s=rdall(fd);
        for line in s.split('\n'){
            let chars:Vec<char>=line.chars().collect();
            let mut start=0;
            while start<chars.len(){
                let end=(start+width).min(chars.len());
                let mut break_at=end;
                if break_spaces&&end<chars.len(){
                    if let Some(p)=chars[start..end].iter().rposition(|&c|c==' '){
                        break_at=start+p+1;
                    }
                }
                for c in &chars[start..break_at]{write(STDOUT,c.to_string().as_bytes());}
                write(STDOUT,b"\n");
                start=break_at;
            }
            if chars.is_empty(){write(STDOUT,b"\n");}
        }
    };
    if files.is_empty(){process(STDIN);}
    else{for f in &files{if f=="-"{process(STDIN);}else{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{continue;}process(fd as i32);close(fd as i32);}}}
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
