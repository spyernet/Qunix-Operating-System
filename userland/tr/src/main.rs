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
    let mut delete = false; let mut squeeze = false; let mut complement = false;
    let mut set1 = String::new(); let mut set2 = String::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-d" | "--delete"    => delete = true,
            "-s" | "--squeeze-repeats" => squeeze = true,
            "-c" | "-C" | "--complement" => complement = true,
            _ => { if set1.is_empty() { set1 = args[i].clone(); } else { set2 = args[i].clone(); } }
        }
        i += 1;
    }
    let s1 = expand_set(&set1);
    let s2 = expand_set(&set2);
    let mut data = alloc::vec![0u8; 65536]; let mut tot=0;
    loop { let n=read(STDIN,&mut data[tot..]); if n<=0{break;} tot+=n as usize; if tot>=data.len(){data.resize(data.len()*2,0);} }
    let mut out = Vec::new();
    let mut prev = 0u8;
    for &b in &data[..tot] {
        let c = b as char;
        let in_set1 = s1.contains(&c);
        let matched = if complement { !in_set1 } else { in_set1 };
        if delete && matched { continue; }
        let mapped = if !delete && matched && !s2.is_empty() {
            let idx = s1.iter().position(|&x| x == c).unwrap_or(0);
            *s2.get(idx).unwrap_or(s2.last().unwrap_or(&c)) as u8
        } else { b };
        if squeeze && mapped == prev && matched { continue; }
        out.push(mapped); prev = mapped;
    }
    write(STDOUT, &out);
    exit(0)
}

fn expand_set(spec: &str) -> Vec<char> {
    let mut result = Vec::new();
    let chars: Vec<char> = spec.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' {
            i+=1;
            if i < chars.len() {
                let c = match chars[i] { 'n'=>'\n','t'=>'\t','r'=>'\r','a'=>'\x07','b'=>'\x08','f'=>'\x0C','v'=>'\x0B',c=>c };
                result.push(c);
            }
        } else if i+2 < chars.len() && chars[i+1] == '-' {
            let start = chars[i]; let end = chars[i+2];
            for c in (start as u32)..=(end as u32) { if let Some(ch)=char::from_u32(c){result.push(ch);} }
            i += 3; continue;
        } else if chars[i] == '[' && i+1 < chars.len() && chars[i+1] == ':' {
            let close = spec[i..].find(":]").map(|p| i+p+2).unwrap_or(i+1);
            let class = &spec[i+2..close-2];
            match class {
                "alpha" => for c in 'a'..='z' { result.push(c); result.push(c.to_uppercase().next().unwrap_or(c)); }
                "digit" => for c in '0'..='9' { result.push(c); }
                "lower" => for c in 'a'..='z' { result.push(c); }
                "upper" => for c in 'A'..='Z' { result.push(c); }
                "space" => for c in [' ','\t','\n','\r'] { result.push(c); }
                "alnum" => { for c in 'a'..='z' { result.push(c); result.push(c.to_uppercase().next().unwrap_or(c)); } for c in '0'..='9' { result.push(c); } }
                "punct" => for c in "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~".chars() { result.push(c); }
                "blank" => { result.push(' '); result.push('\t'); }
                _ => {}
            }
            i = close; continue;
        } else { result.push(chars[i]); }
        i += 1;
    }
    result
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
