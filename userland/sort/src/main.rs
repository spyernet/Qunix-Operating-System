/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

#![no_std]
#![no_main]
#![allow(unused_variables, unused_assignments, unused_mut, dead_code)]
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
    let mut reverse = false;
    let mut unique = false;
    let mut numeric = false;
    let mut human_numeric = false;
    let mut ignore_case = false;
    let mut stable = false;
    let mut key: Option<(usize, usize)> = None;
    let mut field_sep = ' ';
    let mut check = false;
    let mut output: Option<String> = None;
    let mut merge = false;
    let mut random = false;
    let mut files: Vec<String> = Vec::new();
    let mut parallel = 0usize;
    let mut buf_size = 0usize;
    let mut temp_dir = String::from("/tmp");
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-r" | "--reverse"   => reverse = true,
            "-u" | "--unique"    => unique = true,
            "-n" | "--numeric-sort" => numeric = true,
            "-h" | "--human-numeric-sort" => human_numeric = true,
            "-f" | "--ignore-case" => ignore_case = true,
            "-s" | "--stable"    => stable = true,
            "-c" | "--check"     => check = true,
            "-m" | "--merge"     => merge = true,
            "-R" | "--random-sort" => random = true,
            "-t" => { i+=1; if i < args.len() { field_sep = args[i].chars().next().unwrap_or(' '); } }
            "-k" => { i+=1; if i < args.len() { let k = &args[i]; if let Some(c) = k.find(',') { let a: usize = k[..c].parse().unwrap_or(1); let b2: usize = k[c+1..].parse().unwrap_or(a); key = Some((a, b2)); } else { let a: usize = k.parse().unwrap_or(1); key = Some((a, a)); } } }
            "-o" | "--output" => { i+=1; if i < args.len() { output = Some(args[i].clone()); } }
            "--parallel" => { i+=1; parallel = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0); }
            "--buffer-size" => { i+=1; }
            "-T" | "--temporary-directory" => { i+=1; if i < args.len() { temp_dir = args[i].clone(); } }
            "--" => { i+=1; break; }
            s if s.starts_with("-t") => field_sep = s[2..].chars().next().unwrap_or(' '),
            s if s.starts_with('-') && !s.starts_with("--") => {
                for c in s[1..].chars() {
                    match c { 'r'=>reverse=true, 'u'=>unique=true, 'n'=>numeric=true,
                              'f'=>ignore_case=true, 's'=>stable=true, 'c'=>check=true,
                              'h'=>human_numeric=true, 'm'=>merge=true, 'R'=>random=true, _ => {} }
                }
            }
            _ => files.push(args[i].clone()),
        }
        i += 1;
    }
    while i < args.len() { files.push(args[i].clone()); i += 1; }

    // Read all input
    let mut all_lines: Vec<String> = Vec::new();
    let mut read_from = |fd: i32| {
        let mut data = alloc::vec![0u8; 1<<20];
        let mut tot = 0;
        loop { if tot >= data.len() { data.resize(data.len()*2, 0); }
               let n = read(fd, &mut data[tot..]); if n <= 0 { break; } tot += n as usize; }
        let text = String::from_utf8_lossy(&data[..tot]).to_string();
        for line in text.split('\n') {
            if !line.is_empty() { all_lines.push(line.to_string()); }
        }
    };

    if files.is_empty() { read_from(STDIN); }
    else { for f in &files {
        if f == "-" { read_from(STDIN); continue; }
        let mut p = f.clone(); p.push('\0');
        let fd = open(p.as_bytes(), O_RDONLY, 0); if fd < 0 { continue; }
        read_from(fd as i32); close(fd as i32);
    } }

    if check {
        for i in 1..all_lines.len() {
            let cmp = compare_lines(&all_lines[i-1], &all_lines[i], numeric, human_numeric, ignore_case, field_sep, key);
            if cmp == core::cmp::Ordering::Greater { write_err(&alloc::format!("sort: disorder: {}\n", all_lines[i])); exit(1); }
        }
        exit(0);
    }

    // Sort
    all_lines.sort_by(|a, b| {
        let c = compare_lines(a, b, numeric, human_numeric, ignore_case, field_sep, key);
        if reverse { c.reverse() } else { c }
    });

    if unique { all_lines.dedup(); }

    // Output
    let out_fd = if let Some(ref path) = output {
        let mut p = path.clone(); p.push('\0');
        let fd = open(p.as_bytes(), O_WRONLY|O_CREAT|O_TRUNC, 0o644);
        if fd < 0 { STDOUT } else { fd as i32 }
    } else { STDOUT };

    for line in &all_lines {
        write(out_fd, line.as_bytes()); write(out_fd, b"\n");
    }
    exit(0)
}

fn compare_lines(a: &str, b: &str, numeric: bool, human: bool, icase: bool, sep: char, key: Option<(usize,usize)>) -> core::cmp::Ordering {
    let get_field = |line: &str, k: usize| -> String {
        if sep == ' ' { line.split_whitespace().nth(k.saturating_sub(1)).unwrap_or("").to_string() }
        else { line.splitn(k+1, sep).nth(k.saturating_sub(1)).unwrap_or("").to_string() }
    };
    let (ka, kb) = if let Some((k1, _k2)) = key {
        (get_field(a, k1), get_field(b, k1))
    } else { (a.to_string(), b.to_string()) };

    if numeric || human {
        let na = parse_numeric(&ka);
        let nb = parse_numeric(&kb);
        na.partial_cmp(&nb).unwrap_or(core::cmp::Ordering::Equal)
    } else if icase {
        ka.to_lowercase().cmp(&kb.to_lowercase())
    } else {
        ka.cmp(&kb)
    }
}

fn parse_numeric(s: &str) -> f64 {
    let s = s.trim();
    let (s, mult) = if s.ends_with('K') || s.ends_with('k') { (&s[..s.len()-1], 1024f64) }
        else if s.ends_with('M') { (&s[..s.len()-1], 1024f64*1024.0) }
        else if s.ends_with('G') { (&s[..s.len()-1], 1024f64*1024.0*1024.0) }
        else if s.ends_with('T') { (&s[..s.len()-1], 1024f64*1024.0*1024.0*1024.0) }
        else { (s, 1.0) };
    s.parse::<f64>().unwrap_or(0.0) * mult
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
