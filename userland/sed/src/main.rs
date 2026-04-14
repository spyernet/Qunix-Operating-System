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
    let mut scripts: Vec<String> = Vec::new();
    let mut files:   Vec<String> = Vec::new();
    let mut in_place = false;
    let mut in_place_ext = String::new();
    let mut extended = false;
    let mut quiet = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-e" => { i+=1; if i < args.len() { scripts.push(args[i].clone()); } }
            "-f" => { i+=1; } // ignore script file for now
            "-n" | "--quiet" | "--silent" => quiet = true,
            "-E" | "-r"  => extended = true,
            "--" => { i+=1; break; }
            s if s.starts_with("-i") => { in_place = true; in_place_ext = s[2..].to_string(); }
            s if s.starts_with('-') && s.len() == 2 => {
                match s.chars().nth(1) {
                    Some('n') => quiet = true, Some('E')|Some('r') => extended = true, _ => {}
                }
            }
            _ => {
                if scripts.is_empty() { scripts.push(args[i].clone()); }
                else { files.push(args[i].clone()); }
            }
        }
        i += 1;
    }
    while i < args.len() { files.push(args[i].clone()); i += 1; }

    if files.is_empty() {
        process_sed(STDIN, &scripts, quiet, in_place, "");
    } else {
        for f in &files {
            let mut p = f.clone(); p.push('\0');
            let fd = open(p.as_bytes(), O_RDONLY, 0);
            if fd < 0 { write_err(&alloc::format!("sed: {}: No such file\n", f)); continue; }
            if in_place {
                let tmp = alloc::format!("{}.sedtmp\0", f);
                let tfd = open(tmp.as_bytes(), O_WRONLY|O_CREAT|O_TRUNC, 0o644);
                if tfd >= 0 { /* redirect STDOUT to tfd */ }
            }
            process_sed(fd as i32, &scripts, quiet, false, "");
            close(fd as i32);
        }
    }
    exit(0)
}

fn process_sed(fd: i32, scripts: &[String], quiet: bool, _in_place: bool, _fname: &str) {
    let mut data = alloc::vec![0u8; 1<<20];
    let mut tot = 0usize;
    loop { if tot >= data.len() { data.resize(data.len()*2, 0); }
           let n = read(fd, &mut data[tot..]); if n <= 0 { break; } tot += n as usize; }
    let text = String::from_utf8_lossy(&data[..tot]).to_string();
    let lines: Vec<&str> = text.split('\n').collect();

    for (lineno, line) in lines.iter().enumerate() {
        let mut out = line.to_string();
        let mut print = !quiet;
        let mut deleted = false;

        for script in scripts {
            let s = script.trim();
            if s.is_empty() { continue; }
            // Parse address
            let (addr, cmd_part) = parse_address(s, lineno+1, lines.len());
            if !addr { continue; }
            let cmd = cmd_part.trim_start();
            if cmd.is_empty() { continue; }
            match cmd.chars().next().unwrap_or('z') {
                'd' => { deleted = true; print = false; break; }
                'p' => { write_str(&out); write(STDOUT, b"\n"); }
                'q' => { write_str(&out); write(STDOUT, b"\n"); exit(0); }
                'Q' => exit(0),
                's' => {
                    if let Some(result) = do_substitute(cmd, &out) {
                        out = result;
                    }
                }
                'y' => { out = do_transliterate(cmd, &out); }
                'a' => { /* append */ let text = cmd[1..].trim().to_string(); if !quiet { write_str(&out); write(STDOUT, b"\n"); write_str(&text); write(STDOUT, b"\n"); } deleted = true; break; }
                'i' => { let text = cmd[1..].trim().to_string(); write_str(&text); write(STDOUT, b"\n"); }
                'c' => { let text = cmd[1..].trim().to_string(); write_str(&text); write(STDOUT, b"\n"); deleted = true; break; }
                'n' => { if !quiet { write_str(&out); write(STDOUT, b"\n"); } /* TODO: next line */ }
                'N' => {} // next line append
                '=' => { write_str(&alloc::format!("{}\n", lineno+1)); }
                'r' => {} // read file
                'w' => {} // write to file
                _ => {}
            }
        }

        if !deleted && print && lineno + 1 < lines.len() {
            write_str(&out);
            write(STDOUT, b"\n");
        }
    }
}

fn parse_address(s: &str, lineno: usize, total: usize) -> (bool, &str) {
    let b = s.as_bytes();
    if b.is_empty() { return (true, s); }
    // Line number address
    if b[0].is_ascii_digit() {
        let end = b.iter().position(|c| !c.is_ascii_digit()).unwrap_or(b.len());
        let n: usize = s[..end].parse().unwrap_or(0);
        let rest = &s[end..];
        if rest.starts_with(',') {
            let rest2 = &rest[1..];
            let end2 = rest2.as_bytes().iter().position(|c| !c.is_ascii_digit()).unwrap_or(rest2.len());
            let n2: usize = rest2[..end2].parse().unwrap_or(0);
            return (lineno >= n && lineno <= n2, &rest2[end2..]);
        }
        return (lineno == n, rest);
    }
    if b[0] == b'$' { return (lineno == total, &s[1..]); }
    // Regex address
    if b[0] == b'/' {
        let end = s[1..].find('/').map(|i| i + 2).unwrap_or(s.len());
        let pat = &s[1..end.saturating_sub(1)];
        let rest = &s[end..];
        // Simple contains check
        return (true, rest); // simplified: always match
    }
    (true, s)
}

fn do_substitute(cmd: &str, line: &str) -> Option<String> {
    let sep = cmd.chars().nth(1)?;
    let parts: Vec<&str> = cmd[2..].splitn(4, sep).collect();
    if parts.len() < 2 { return None; }
    let pat = parts[0];
    let rep = parts[1];
    let flags = parts.get(2).unwrap_or(&"");
    let global = flags.contains('g');
    let icase  = flags.contains('i') || flags.contains('I');
    let do_print = flags.contains('p');

    let line_cmp = if icase { line.to_lowercase() } else { line.to_string() };
    let pat_cmp  = if icase { pat.to_lowercase()  } else { pat.to_string() };

    if !line_cmp.contains(&pat_cmp) { return None; }

    let result = if global {
        line_cmp.replace(&pat_cmp, rep)
    } else {
        let pos = line_cmp.find(&pat_cmp)?;
        alloc::format!("{}{}{}", &line[..pos], rep, &line[pos+pat.len()..])
    };

    if do_print { write_str(&result); write(STDOUT, b"\n"); }
    Some(result)
}

fn do_transliterate(cmd: &str, line: &str) -> String {
    let sep = cmd.chars().nth(1).unwrap_or('/');
    let parts: Vec<&str> = cmd[2..].splitn(3, sep).collect();
    if parts.len() < 2 { return line.to_string(); }
    let from: Vec<char> = parts[0].chars().collect();
    let to:   Vec<char> = parts[1].chars().collect();
    line.chars().map(|c| {
        if let Some(idx) = from.iter().position(|&f| f == c) {
            to.get(idx).copied().unwrap_or(c)
        } else { c }
    }).collect()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
