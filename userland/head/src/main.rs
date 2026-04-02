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

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let args = args_from_argv(argc, argv);
    let mut n_lines: i64 = 10; let mut n_bytes: i64 = -1; let mut quiet=false; let mut verbose=false;
    let mut files: Vec<String> = Vec::new();
    let mut i=1;
    while i < args.len() {
        match args[i].as_str() {
            "-q"|"--quiet"|"--silent" => quiet=true, "-v"|"--verbose" => verbose=true,
            "-n"|"--lines" => { i+=1; if i<args.len() { let s=&args[i]; n_lines=if s.starts_with('-'){-(s[1..].parse::<i64>().unwrap_or(0))}else{s.parse().unwrap_or(10)}; } }
            "-c"|"--bytes" => { i+=1; if i<args.len() { n_bytes=args[i].parse().unwrap_or(-1); } }
            s if s.starts_with("-n") => { n_lines=s[2..].parse().unwrap_or(10); }
            s if s.starts_with("-c") => { n_bytes=s[2..].parse().unwrap_or(-1); }
            s if s.starts_with('-') && s[1..].chars().all(|c| c.is_ascii_digit()) => { n_lines=s[1..].parse().unwrap_or(10); }
            _ => files.push(args[i].clone()),
        }
        i+=1;
    }
    let multiple = files.len()>1;
    let do_file = |fd:i32, name:&str| {
        if (multiple || verbose) && !quiet { write_str(&alloc::format!("==> {} <==\n",name)); }
        if n_bytes>0 {
            let mut rem=n_bytes as usize; let mut buf=[0u8;4096];
            while rem>0 { let n=read(fd,&mut buf[..rem.min(4096)]); if n<=0{break;} write(STDOUT,&buf[..n as usize]); rem-=n as usize; }
        } else if n_lines>=0 {
            let mut lines_left=n_lines as u64; let mut buf=[0u8;4096];
            'outer: loop { let n=read(fd,&mut buf); if n<=0{break;} for i in 0..n as usize { write(STDOUT,&buf[i..i+1]); if buf[i]==b'\n' { lines_left-=1; if lines_left==0{break 'outer;} } } }
        } else {
            // Negative: print all except last |n| lines
            let mut data=alloc::vec![0u8;1<<20]; let mut tot=0;
            loop{let n=read(fd,&mut data[tot..]); if n<=0{break;} tot+=n as usize; if tot>=data.len(){data.resize(data.len()*2,0);}}
            let text=String::from_utf8_lossy(&data[..tot]).to_string();
            let lines: Vec<&str>=text.split('\n').collect();
            let skip=(-n_lines) as usize; let end=lines.len().saturating_sub(skip);
            for l in &lines[..end] { write(STDOUT,l.as_bytes()); write(STDOUT,b"\n"); }
        }
    };
    if files.is_empty() { do_file(STDIN,"(stdin)"); }
    else { for (idx,f) in files.iter().enumerate() {
        if f=="-" { do_file(STDIN,"-"); }
        else { let mut p=f.clone(); p.push('\0'); let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{write_err(&alloc::format!("head: {}: cannot open\n",f));continue;} do_file(fd as i32,f); close(fd as i32); }
        if multiple && !quiet && idx+1<files.len() { write(STDOUT,b"\n"); }
    } }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
