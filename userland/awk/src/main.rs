/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

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
    let mut program = String::new();
    let mut files: Vec<String> = Vec::new();
    let mut fs = " ".to_string();
    let mut vars: Vec<(String, String)> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-f" => { i+=1; } // ignore program file
            "-F" => { i+=1; if i < args.len() { fs = args[i].clone(); } }
            "-v" => { i+=1; if i < args.len() { if let Some(eq)=args[i].find('=') { vars.push((args[i][..eq].to_string(), args[i][eq+1..].to_string())); } } }
            "--" => { i+=1; break; }
            s if s.starts_with("-F") => fs = s[2..].to_string(),
            s if s.starts_with("-v") => { if let Some(eq) = s[2..].find('=') { vars.push((s[2..2+eq].to_string(), s[2+eq+1..].to_string())); } }
            _ => { if program.is_empty() { program = args[i].clone(); } else { files.push(args[i].clone()); } }
        }
        i += 1;
    }
    while i < args.len() { files.push(args[i].clone()); i += 1; }

    run_awk(&program, &files, &fs, &vars);
    exit(0)
}

fn run_awk(prog: &str, files: &[String], fs: &str, vars: &[(String,String)]) {
    // Parse program into (pattern, action) pairs
    let rules = parse_awk_program(prog);

    // Collect input
    let process = |fd: i32, fname: &str| {
        let mut data = alloc::vec![0u8; 1<<20];
        let mut tot = 0;
        loop { if tot >= data.len() { data.resize(data.len()*2, 0); }
               let n = read(fd, &mut data[tot..]); if n <= 0 { break; } tot += n as usize; }
        let text = String::from_utf8_lossy(&data[..tot]).to_string();
        let mut fnr = 0u64;
        for line in text.split('\n') {
            if line.is_empty() { continue; }
            fnr += 1;
            exec_awk_rules(&rules, line, fnr, fs, fname);
        }
    };

    // BEGIN rules
    for (pat, action) in &rules {
        if pat == "BEGIN" { exec_awk_action(action, "", 0, fs, &[]); }
    }

    if files.is_empty() {
        process(STDIN, "(stdin)");
    } else {
        for f in files {
            if f == "-" { process(STDIN, f); continue; }
            let mut p = f.clone(); p.push('\0');
            let fd = open(p.as_bytes(), O_RDONLY, 0);
            if fd < 0 { write_err(&alloc::format!("awk: {}: No such file\n", f)); continue; }
            process(fd as i32, f);
            close(fd as i32);
        }
    }

    // END rules
    for (pat, action) in &rules {
        if pat == "END" { exec_awk_action(action, "", 0, fs, &[]); }
    }
}

fn parse_awk_program(prog: &str) -> Vec<(String, String)> {
    let mut rules = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = prog.chars().collect();
    while i < chars.len() {
        while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t' || chars[i] == '\n') { i+=1; }
        if i >= chars.len() { break; }
        // Parse pattern
        let mut pat = String::new();
        if chars[i] == '{' {
            // No pattern, just action
        } else {
            while i < chars.len() && chars[i] != '{' && chars[i] != '\n' { pat.push(chars[i]); i+=1; }
        }
        pat = pat.trim().to_string();
        // Parse action
        let mut action = String::new();
        if i < chars.len() && chars[i] == '{' {
            i+=1;
            let mut depth = 1;
            while i < chars.len() && depth > 0 {
                if chars[i] == '{' { depth+=1; }
                if chars[i] == '}' { depth-=1; if depth==0 { i+=1; break; } }
                action.push(chars[i]); i+=1;
            }
        }
        if !pat.is_empty() || !action.is_empty() { rules.push((pat, action)); }
    }
    rules
}

fn exec_awk_rules(rules: &[(String,String)], line: &str, nr: u64, fs: &str, fname: &str) {
    let fields = split_fields(line, fs);
    for (pat, action) in rules {
        if pat == "BEGIN" || pat == "END" { continue; }
        let matches = if pat.is_empty() { true }
            else if pat.starts_with('/') && pat.ends_with('/') { line.contains(&pat[1..pat.len()-1]) }
            else { true }; // simplified
        if matches { exec_awk_action(action, line, nr, fs, &fields); }
    }
}

fn split_fields(line: &str, fs: &str) -> Vec<String> {
    if fs == " " {
        line.split_whitespace().map(|s| s.to_string()).collect()
    } else {
        line.split(fs).map(|s| s.to_string()).collect()
    }
}

fn exec_awk_action(action: &str, line: &str, nr: u64, fs: &str, fields: &[String]) {
    let action = action.trim();
    // Handle common awk patterns
    for stmt in action.split(';') {
        let stmt = stmt.trim();
        if stmt.starts_with("print") {
            let what = stmt.trim_start_matches("print").trim();
            if what.is_empty() || what == "$0" { write_str(line); write(STDOUT, b"\n"); }
            else {
                // Evaluate fields
                let out = eval_awk_expr(what, line, fields, nr);
                write_str(&out); write(STDOUT, b"\n");
            }
        } else if stmt.starts_with("printf") {
            let rest = stmt[6..].trim();
            // Very simplified printf
            let out = eval_awk_expr(rest, line, fields, nr);
            write_str(&out);
        } else if stmt.starts_with("next") {
            return;
        }
    }
}

fn eval_awk_expr(expr: &str, line: &str, fields: &[String], nr: u64) -> String {
    let expr = expr.trim();
    match expr {
        "$0" => line.to_string(),
        "NR" => nr.to_string(),
        "NF" => fields.len().to_string(),
        e if e.starts_with('$') => {
            let idx: usize = e[1..].parse().unwrap_or(0);
            if idx == 0 { line.to_string() }
            else { fields.get(idx.saturating_sub(1)).cloned().unwrap_or_default() }
        }
        e if e.starts_with('"') && e.ends_with('"') => e[1..e.len()-1].to_string(),
        _ => {
            // Try field concatenation with comma
            expr.split(',').map(|part| {
                let part = part.trim();
                eval_awk_expr(part, line, fields, nr)
            }).collect::<Vec<_>>().join(" ")
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
