#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;


fn parse_argv(argc: u64, ap: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *ap.add(i); let mut n=0; while *p.add(n)!=0{n+=1;}
        String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).into_owned()
    }).collect()
}
fn w(s: &str) { write(STDOUT, s.as_bytes()); }
fn e(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> alloc::vec::Vec<u8> {
    let mut d=alloc::vec![0u8;1<<20]; let mut t=0;
    loop { if t>=d.len(){d.resize(d.len()*2,0);} let n=read(fd,&mut d[t..]); if n<=0{break;} t+=n as usize; }
    d.truncate(t); d
}
fn rdfile(path: &str) -> alloc::vec::Vec<u8> {
    let mut p=path.to_string(); p.push('\0');
    let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{return alloc::vec![];}
    let d=rdall(fd as i32); close(fd as i32); d
}
fn cstr(p: *const u8) -> String {
    unsafe { let mut n=0; while *p.add(n)!=0{n+=1;}
    String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).into_owned() }
}

#[no_mangle] #[link_section=".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = parse_argv(argc, argv);
    let mut by_bytes = false; let mut by_chars = false; let mut by_fields = false;
    let mut ranges = String::new(); let mut delim = '\t';
    let mut only_delim = false; let mut complement = false; let mut zero = false;
    let mut output_delim: Option<String> = None;
    let mut files: Vec<String> = Vec::new();
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "-b" | "--bytes"      => { i+=1; by_bytes=true; ranges=a.get(i).cloned().unwrap_or_default(); }
            "-c" | "--characters" => { i+=1; by_chars=true; ranges=a.get(i).cloned().unwrap_or_default(); }
            "-f" | "--fields"     => { i+=1; by_fields=true; ranges=a.get(i).cloned().unwrap_or_default(); }
            "-d" | "--delimiter"  => { i+=1; delim=a.get(i).and_then(|s|s.chars().next()).unwrap_or('\t'); }
            "-s" | "--only-delimited" => only_delim=true,
            "--complement"         => complement=true,
            "-z" | "--zero-terminated" => zero=true,
            "--output-delimiter"   => { i+=1; output_delim=a.get(i).cloned(); }
            "--help" => { w("Usage: cut OPTION... [FILE]...\n"); exit(0); }
            "--version" => { w("cut (Qunix) 1.0\n"); exit(0); }
            s if s.starts_with("-b") => { by_bytes=true; ranges=s[2..].to_string(); }
            s if s.starts_with("-c") => { by_chars=true; ranges=s[2..].to_string(); }
            s if s.starts_with("-f") => { by_fields=true; ranges=s[2..].to_string(); }
            s if s.starts_with("-d") => { delim=s[2..].chars().next().unwrap_or('\t'); }
            "--" => { i+=1; break; }
            _ => files.push(a[i].clone()),
        }
        i += 1;
    }
    while i < a.len() { files.push(a[i].clone()); i+=1; }
    if !by_bytes && !by_chars && !by_fields {
        e("cut: you must specify a list of bytes, characters, or fields\n");
        exit(1);
    }
    let positions = parse_ranges(&ranges);
    let out_delim = output_delim.unwrap_or_else(|| delim.to_string());
    let sep = if zero { b'\0' } else { b'\n' };
    let process = |fd: i32| {
        let data = rdall(fd);
        let text = String::from_utf8_lossy(&data);
        for line in text.split('\n') {
            if line.is_empty() { continue; }
            if by_fields {
                let parts: Vec<&str> = line.split(delim).collect();
                if only_delim && !line.contains(delim) { write(STDOUT, &[sep]); continue; }
                let selected: Vec<&str> = if complement {
                    parts.iter().enumerate().filter(|(i,_)|!positions.contains(&(i+1))).map(|(_,s)|*s).collect()
                } else {
                    let mut r = Vec::new();
                    for &p in &positions { if let Some(s) = parts.get(p.saturating_sub(1)) { r.push(*s); } }
                    r
                };
                w(&selected.join(&out_delim));
            } else {
                let chars: Vec<char> = line.chars().collect();
                let selected: String = if complement {
                    chars.iter().enumerate().filter(|(i,_)|!positions.contains(&(i+1))).map(|(_,c)|*c).collect()
                } else {
                    let mut r = String::new();
                    for &p in &positions { if let Some(&c) = chars.get(p.saturating_sub(1)) { r.push(c); } }
                    r
                };
                w(&selected);
            }
            write(STDOUT, &[sep]);
        }
    };
    if files.is_empty() { process(STDIN); }
    else { for f in &files {
        if f=="-" { process(STDIN); continue; }
        let mut p=f.clone(); p.push('\0');
        let fd=open(p.as_bytes(),O_RDONLY,0); if fd<0{e(&alloc::format!("cut: {}: No such file\n",f));continue;}
        process(fd as i32); close(fd as i32);
    }}
    exit(0)
}
fn parse_ranges(spec: &str) -> Vec<usize> {
    let mut r = Vec::new();
    for part in spec.split(',') {
        let part=part.trim(); if part.is_empty(){continue;}
        if let Some(dash) = part.find('-') {
            let start=if dash==0{1}else{part[..dash].parse().unwrap_or(1)};
            let end=if dash==part.len()-1{usize::MAX}else{part[dash+1..].parse().unwrap_or(start)};
            for n in start..=end.min(65535){r.push(n);}
        } else if let Ok(n)=part.parse::<usize>(){r.push(n);}
    }
    r.sort(); r.dedup(); r
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
