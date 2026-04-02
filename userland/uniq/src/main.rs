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
    let mut count = false; let mut repeated = false; let mut unique = false;
    let mut ignore_case = false; let mut skip_fields = 0usize; let mut skip_chars = 0usize;
    let mut files: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--count" => count = true,
            "-d" | "--repeated" => repeated = true,
            "-u" | "--unique" => unique = true,
            "-i" | "--ignore-case" => ignore_case = true,
            "-f" => { i+=1; skip_fields = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0); }
            "-s" => { i+=1; skip_chars = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0); }
            s if s.starts_with("-f") => skip_fields = s[2..].parse().unwrap_or(0),
            s if s.starts_with("-s") => skip_chars = s[2..].parse().unwrap_or(0),
            _ => files.push(args[i].clone()),
        }
        i += 1;
    }
    let (in_fd, out_fd) = match files.len() {
        0 => (STDIN, STDOUT),
        1 => { let mut p=files[0].clone(); p.push('\0'); let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{exit(1)}; (fd as i32, STDOUT) }
        _ => { let mut p=files[0].clone(); p.push('\0'); let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{exit(1)};
               let mut p2=files[1].clone(); p2.push('\0'); let fd2=open(p2.as_bytes(),O_WRONLY|O_CREAT|O_TRUNC,0o644);
               (fd as i32, if fd2<0{STDOUT} else{fd2 as i32}) }
    };
    let mut data = alloc::vec![0u8; 1<<20]; let mut tot=0;
    loop { if tot>=data.len(){data.resize(data.len()*2,0);} let n=read(in_fd,&mut data[tot..]); if n<=0{break;} tot+=n as usize; }
    let text = String::from_utf8_lossy(&data[..tot]).to_string();
    let lines: Vec<&str> = text.split('\n').collect();

    let cmp_key = |line: &str| -> String {
        let mut s = line.to_string();
        if skip_fields > 0 { s = s.splitn(skip_fields+1, ' ').last().unwrap_or("").to_string(); }
        if skip_chars > 0 { s = s.chars().skip(skip_chars).collect(); }
        if ignore_case { s = s.to_lowercase(); }
        s
    };

    let mut prev_key = String::new(); let mut prev_line = String::new(); let mut cnt = 0u64;
    for line in lines {
        if line.is_empty() { continue; }
        let key = cmp_key(line);
        if key == prev_key { cnt += 1; }
        else {
            if cnt > 0 {
                let should_print = if repeated { cnt > 1 } else if unique { cnt == 1 } else { true };
                if should_print {
                    if count { write(out_fd, alloc::format!("{:7} ", cnt).as_bytes()); }
                    write(out_fd, prev_line.as_bytes()); write(out_fd, b"\n");
                }
            }
            prev_key = key; prev_line = line.to_string(); cnt = 1;
        }
    }
    if cnt > 0 {
        let should_print = if repeated { cnt > 1 } else if unique { cnt == 1 } else { true };
        if should_print {
            if count { write(out_fd, alloc::format!("{:7} ", cnt).as_bytes()); }
            write(out_fd, prev_line.as_bytes()); write(out_fd, b"\n");
        }
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
