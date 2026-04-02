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
    let mut files: Vec<String>=Vec::new();let mut i=1;
    while i<a.len(){match a[i].as_str(){s if s.starts_with('-')=>{},_=>files.push(a[i].clone())}i+=1;}
    let pager=|fd:i32,name:&str|{
        let text=rdall(fd);let lines: Vec<&str>=text.split('\n').collect();
        let rows=24usize;let mut row=0;
        while row<lines.len(){
            let end=(row+rows).min(lines.len());
            for l in &lines[row..end]{wstr(l);write(STDOUT,b"\n");}
            row=end;
            if row<lines.len(){
                write(STDERR,&alloc::format!("\x1b[7m:{}\x1b[m",if lines.len()>0{(row*100/lines.len()) as u8}else{100u8}).as_bytes());
                let mut b=[0u8;4];let n=read(STDIN,&mut b);
                write(STDERR,b"\r\x1b[K");
                if n>0{match b[0]{b'q'|3=>break,b' '=>{},b'b'=>{row=row.saturating_sub(rows*2);}  ,_=>{}}}
            }
        }
    };
    if files.is_empty(){pager(STDIN,"(stdin)");}
    else{for f in &files{if f=="-"{pager(STDIN,"-");}else{let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{werr(&alloc::format!("{}: No such file\n",f));continue;}pager(fd as i32,f);close(fd as i32);}}}
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
