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
fn cstr_str(p: *const u8) -> String {
    unsafe {
        let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }
}

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let args = args_from_argv(argc, argv);
    let mut pat = String::new();
    let mut files: Vec<String> = Vec::new();
    let mut invert = false;
    let mut ignore_case = false;
    let mut line_num = false;
    let mut count_only = false;
    let mut files_with = false;
    let mut files_without = false;
    let mut quiet = false;
    let mut whole_word = false;
    let mut whole_line = false;
    let mut color = true;
    let mut extended = false;
    let mut fixed = false;
    let mut recursive = false;
    let mut max_count: usize = usize::MAX;
    let mut before_ctx = 0usize;
    let mut after_ctx = 0usize;
    let mut patterns: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-v" | "--invert-match"  => invert = true,
            "-i" | "--ignore-case"   => ignore_case = true,
            "-n" | "--line-number"   => line_num = true,
            "-c" | "--count"         => count_only = true,
            "-l" | "--files-with-matches" => files_with = true,
            "-L" | "--files-without-match" => files_without = true,
            "-q" | "--quiet" | "--silent" => quiet = true,
            "-w" | "--word-regexp"   => whole_word = true,
            "-x" | "--line-regexp"   => whole_line = true,
            "--color=never"          => color = false,
            "--color=always" | "--color=auto" | "--color" => color = true,
            "-E" | "--extended-regexp" => extended = true,
            "-F" | "--fixed-strings"   => fixed = true,
            "-r" | "-R" | "--recursive" => recursive = true,
            "-e" => { i += 1; if i < args.len() { patterns.push(args[i].clone()); } }
            "-f" => { i += 1; /* pattern file — skip */ }
            "-m" => { i += 1; max_count = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX); }
            "-A" => { i += 1; after_ctx = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0); }
            "-B" => { i += 1; before_ctx = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0); }
            "-C" | "--context" => { i += 1; let n = args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0); before_ctx = n; after_ctx = n; }
            "--" => { i += 1; break; }
            s if s.starts_with("--context=") => { let n: usize = s[10..].parse().unwrap_or(0); before_ctx=n; after_ctx=n; }
            s if s.starts_with("--after-context=")  => { after_ctx  = s[16..].parse().unwrap_or(0); }
            s if s.starts_with("--before-context=") => { before_ctx = s[17..].parse().unwrap_or(0); }
            s if s.starts_with("--max-count=") => { max_count = s[12..].parse().unwrap_or(usize::MAX); }
            s if s.starts_with('-') && !s.starts_with("--") => {
                let b = s[1..].as_bytes();
                let mut j = 0;
                while j < b.len() {
                    match b[j] {
                        b'v' => invert=true, b'i' => ignore_case=true, b'n' => line_num=true,
                        b'c' => count_only=true, b'l' => files_with=true, b'L' => files_without=true,
                        b'q' => quiet=true, b'w' => whole_word=true, b'x' => whole_line=true,
                        b'E' => extended=true, b'F' => fixed=true, b'r'|b'R' => recursive=true,
                        b'e' => { let rest = String::from_utf8_lossy(&b[j+1..]).to_string();
                            if !rest.is_empty() { patterns.push(rest); } else { i+=1; if i < args.len() { patterns.push(args[i].clone()); } } break; }
                        _ => {}
                    }
                    j += 1;
                }
            }
            _ => {
                if patterns.is_empty() && pat.is_empty() { pat = a.clone(); }
                else { files.push(a.clone()); }
            }
        }
        i += 1;
    }
    while i < args.len() { files.push(args[i].clone()); i += 1; }
    if patterns.is_empty() { patterns.push(pat); }

    let show_filename = files.len() > 1 || recursive;
    let mut found_any = false;

    if files.is_empty() {
        let r = grep_fd(STDIN, &patterns, invert, ignore_case, line_num, count_only,
                       quiet, whole_word, whole_line, color, max_count,
                       before_ctx, after_ctx, if show_filename { Some("(stdin)") } else { None });
        if r { found_any = true; }
    } else {
        for f in &files {
            if recursive {
                grep_recursive(f, &patterns, invert, ignore_case, line_num, count_only,
                              quiet, whole_word, whole_line, color, max_count, before_ctx, after_ctx,
                              files_with, files_without, &mut found_any);
            } else {
                let mut p = f.clone(); p.push('\0');
                let fd = open(p.as_bytes(), O_RDONLY, 0);
                if fd < 0 { write_err(&alloc::format!("grep: {}: No such file or directory\n", f)); continue; }
                let label = if show_filename { Some(f.as_str()) } else { None };
                let r = grep_fd(fd as i32, &patterns, invert, ignore_case, line_num, count_only,
                               quiet, whole_word, whole_line, color, max_count,
                               before_ctx, after_ctx, label);
                close(fd as i32);
                if files_with && r { write_str(&alloc::format!("{}\n", f)); found_any = true; }
                else if files_without && !r { write_str(&alloc::format!("{}\n", f)); }
                else if !files_with && !files_without && r { found_any = true; }
            }
        }
    }
    exit(if found_any { 0 } else { 1 })
}

fn grep_recursive(path: &str, patterns: &[String], invert: bool, icase: bool, lnum: bool,
                  count: bool, quiet: bool, wword: bool, wline: bool, color: bool,
                  max: usize, bctx: usize, actx: usize, fw: bool, fnw: bool, found: &mut bool) {
    let mut st = [0u64; 22];
    let mut p = path.to_string(); p.push('\0');
    unsafe { syscall::syscall2(4, p.as_ptr() as u64, st.as_mut_ptr() as u64) };
    let mode = (st[2] >> 32) as u32; // approximate
    let fd = open(p.as_bytes(), 0o200000, 0);
    if fd >= 0 {
        let mut buf = alloc::vec![0u8; 32768];
        let mut entries = Vec::new();
        loop {
            let n = getdents64(fd as i32, &mut buf);
            if n <= 0 { break; }
            let mut off = 0;
            while off < n as usize {
                let reclen = u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
                let name = &buf[off+19..]; let nlen = name.iter().position(|&b| b==0).unwrap_or(0);
                let name_s = String::from_utf8_lossy(&name[..nlen]).to_string();
                if name_s != "." && name_s != ".." { entries.push(name_s); }
                if reclen == 0 { break; } off += reclen;
            }
        }
        close(fd as i32);
        for e in entries {
            let sub = alloc::format!("{}/{}", path, e);
            grep_recursive(&sub, patterns, invert, icase, lnum, count, quiet, wword, wline, color, max, bctx, actx, fw, fnw, found);
        }
    } else {
        let fd = open(p.as_bytes(), O_RDONLY, 0);
        if fd < 0 { return; }
        let r = grep_fd(fd as i32, patterns, invert, icase, lnum, count, quiet, wword, wline, color, max, bctx, actx, Some(path));
        close(fd as i32);
        if r { *found = true; }
    }
}

fn grep_fd(fd: i32, patterns: &[String], invert: bool, icase: bool, lnum: bool, count_only: bool,
           quiet: bool, wword: bool, wline: bool, color: bool, max: usize,
           bctx: usize, actx: usize, label: Option<&str>) -> bool {
    let mut all_data = alloc::vec![0u8; 1 << 20]; // 1MB
    let mut total = 0usize;
    loop {
        if total >= all_data.len() { all_data.resize(all_data.len() * 2, 0); }
        let n = read(fd, &mut all_data[total..]);
        if n <= 0 { break; }
        total += n as usize;
    }
    let text = String::from_utf8_lossy(&all_data[..total]);
    let lines: Vec<&str> = text.split('\n').collect();
    let mut matched_count = 0u64;
    let mut found = false;
    let mut count_found = 0usize;
    let mut context_buf: alloc::collections::VecDeque<String> = alloc::collections::VecDeque::new();

    for (lineno, line) in lines.iter().enumerate() {
        let match_line = if icase { line.to_lowercase() } else { line.to_string() };
        let mut matched = patterns.iter().any(|p| {
            let pat = if icase { p.to_lowercase() } else { p.clone() };
            if wline { match_line == pat }
            else if wword { match_line.split_whitespace().any(|w| w == pat) }
            else { match_line.contains(&pat) }
        });
        if invert { matched = !matched; }
        if matched {
            if quiet { return true; }
            count_found += 1;
            found = true;
            if count_found > max { break; }
            if !count_only {
                // Print before context
                let ctx_start = context_buf.len().saturating_sub(bctx);
                for ctx_line in context_buf.iter().skip(ctx_start) {
                    if let Some(lbl) = label { write_str(&alloc::format!("{}-", lbl)); }
                    write_str(ctx_line); write(STDOUT, b"\n");
                }
                if let Some(lbl) = label { write_str(&alloc::format!("{}:", lbl)); }
                if lnum { write_str(&alloc::format!("{}:", lineno + 1)); }
                // Highlight match
                if color {
                    let mut printed = false;
                    for pat in patterns {
                        let search = if icase { line.to_lowercase() } else { line.to_string() };
                        let pat_s = if icase { pat.to_lowercase() } else { pat.clone() };
                        if let Some(pos) = search.find(&pat_s) {
                            write(STDOUT, line[..pos].as_bytes());
                            write_str("\x1b[1;31m");
                            write(STDOUT, line[pos..pos+pat_s.len()].as_bytes());
                            write_str("\x1b[0m");
                            write(STDOUT, line[pos+pat_s.len()..].as_bytes());
                            printed = true; break;
                        }
                    }
                    if !printed { write(STDOUT, line.as_bytes()); }
                } else { write(STDOUT, line.as_bytes()); }
                write(STDOUT, b"\n");
            }
            context_buf.clear();
        } else {
            if bctx > 0 {
                if context_buf.len() >= bctx { context_buf.pop_front(); }
                context_buf.push_back(line.to_string());
            }
        }
    }
    if count_only {
        if let Some(lbl) = label { write_str(&alloc::format!("{}:", lbl)); }
        write_str(&alloc::format!("{}\n", count_found));
    }
    found
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
