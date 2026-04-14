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
    let mut roots: Vec<String> = Vec::new();
    let mut predicates: Vec<Predicate> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "." | "/" | ".." => roots.push(args[i].clone()),
            s if !s.starts_with('-') && !s.starts_with('(') && !s.starts_with('!') && roots.is_empty() => roots.push(s.to_string()),
            "-name"  => { i+=1; predicates.push(Predicate::Name(args.get(i).cloned().unwrap_or_default())); }
            "-iname" => { i+=1; predicates.push(Predicate::IName(args.get(i).cloned().unwrap_or_default())); }
            "-type"  => { i+=1; predicates.push(Predicate::Type(args.get(i).and_then(|s| s.chars().next()).unwrap_or('f'))); }
            "-maxdepth" => { i+=1; predicates.push(Predicate::MaxDepth(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX))); }
            "-mindepth" => { i+=1; predicates.push(Predicate::MinDepth(args.get(i).and_then(|s| s.parse().ok()).unwrap_or(0))); }
            "-size"  => { i+=1; predicates.push(Predicate::Size(args.get(i).cloned().unwrap_or_default())); }
            "-mtime" | "-atime" | "-ctime" => {
                let kind = args[i].clone(); i+=1;
                predicates.push(Predicate::Time(kind, args.get(i).and_then(|s| s.parse::<i64>().ok()).unwrap_or(0)));
            }
            "-newer" => { i+=1; predicates.push(Predicate::Newer(args.get(i).cloned().unwrap_or_default())); }
            "-perm"  => { i+=1; predicates.push(Predicate::Perm(args.get(i).cloned().unwrap_or_default())); }
            "-user"  => { i+=1; predicates.push(Predicate::User(args.get(i).cloned().unwrap_or_default())); }
            "-group" => { i+=1; predicates.push(Predicate::Group(args.get(i).cloned().unwrap_or_default())); }
            "-print" | "-print0" => predicates.push(Predicate::Print(args[i].ends_with('0'))),
            "-ls"    => predicates.push(Predicate::Ls),
            "-exec"  => {
                let mut cmd_parts = Vec::new(); i+=1;
                while i < args.len() && args[i] != ";" { cmd_parts.push(args[i].clone()); i+=1; }
                predicates.push(Predicate::Exec(cmd_parts));
            }
            "-delete" => predicates.push(Predicate::Delete),
            "-empty"  => predicates.push(Predicate::Empty),
            "-not" | "!" => predicates.push(Predicate::Not),
            "-and" | "-a" | "-o" | "-or" => {}
            "-follow" | "-L" => {}
            "-xdev"   => {}
            "-prune"  => predicates.push(Predicate::Prune),
            _ => {}
        }
        i += 1;
    }
    if roots.is_empty() { roots.push(".".to_string()); }
    let max_depth = predicates.iter().find_map(|p| if let Predicate::MaxDepth(d) = p { Some(*d) } else { None }).unwrap_or(usize::MAX);
    let min_depth = predicates.iter().find_map(|p| if let Predicate::MinDepth(d) = p { Some(*d) } else { None }).unwrap_or(0);

    for root in &roots {
        find_recurse(root, &predicates, 0, max_depth, min_depth);
    }
    exit(0)
}

#[derive(Clone)]
enum Predicate {
    Name(String), IName(String), Type(char), MaxDepth(usize), MinDepth(usize),
    Size(String), Time(String, i64), Newer(String), Perm(String), User(String), Group(String),
    Print(bool), Ls, Exec(Vec<String>), Delete, Empty, Not, Prune,
}

fn find_recurse(path: &str, preds: &[Predicate], depth: usize, max: usize, min: usize) {
    if depth > max { return; }
    let mut st = [0u64; 22]; let mut p = path.to_string(); p.push('\0');
    unsafe { syscall::syscall2(4, p.as_ptr() as u64, st.as_mut_ptr() as u64) };
    let mode = (st[2] >> 32) as u32;
    let size = st[7] as i64; let mtime = st[11] as i64;
    let is_dir = mode & 0xF000 == 0x4000;
    let name = path.rsplit('/').next().unwrap_or(path);

    if depth >= min {
        if matches_preds(path, name, mode, size, mtime, preds) {
            let print_null = preds.iter().any(|p| matches!(p, Predicate::Print(true)));
            let has_print  = preds.iter().any(|p| matches!(p, Predicate::Print(_)));
            let has_exec   = preds.iter().any(|p| matches!(p, Predicate::Exec(_)));
            let has_delete = preds.iter().any(|p| matches!(p, Predicate::Delete));
            let has_ls     = preds.iter().any(|p| matches!(p, Predicate::Ls));

            if has_delete {
                if is_dir { let mut p=path.to_string(); p.push('\0'); unsafe{syscall::syscall1(84,p.as_ptr() as u64)}; }
                else { let mut p=path.to_string(); p.push('\0'); unsafe{syscall::syscall1(87,p.as_ptr() as u64)}; }
            } else if has_ls {
                write_str(&alloc::format!("{:8} {:8} {} {}\n", st[0], size, format_mode_find(mode), path));
            } else if has_exec {
                for pred in preds { if let Predicate::Exec(parts) = pred {
                    exec_find_cmd(parts, path);
                }}
            } else {
                write_str(path);
                if print_null { write(STDOUT, b"\0"); } else { write(STDOUT, b"\n"); }
            }
        }
    }

    if is_dir && depth < max {
        let fd = open(p.as_bytes(), 0o200000, 0);
        if fd < 0 { return; }
        let mut buf = alloc::vec![0u8; 32768];
        let mut entries = Vec::new();
        loop { let n=getdents64(fd as i32,&mut buf); if n<=0{break;}
            let mut off=0;
            while off < n as usize {
                let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
                let name=&buf[off+19..]; let nlen=name.iter().position(|&b|b==0).unwrap_or(0);
                let name_s=String::from_utf8_lossy(&name[..nlen]).to_string();
                if name_s!="." && name_s!=".." { entries.push(name_s); }
                if reclen==0{break;} off+=reclen;
            }
        }
        close(fd as i32);
        entries.sort();
        for e in entries {
            let sub = if path == "." { e.clone() } else { alloc::format!("{}/{}", path, e) };
            find_recurse(&sub, preds, depth+1, max, min);
        }
    }
}

fn matches_preds(path: &str, name: &str, mode: u32, size: i64, mtime: i64, preds: &[Predicate]) -> bool {
    let now = unsafe { syscall::syscall0(39) } as i64 * 86400;
    let is_dir = mode & 0xF000 == 0x4000;
    let mut negate = false;
    for pred in preds {
        let result = match pred {
            Predicate::Name(pat)  => glob_match(pat, name),
            Predicate::IName(pat) => glob_match(&pat.to_lowercase(), &name.to_lowercase()),
            Predicate::Type(t)    => match t { 'f'=>mode&0xF000==0x8000, 'd'=>is_dir, 'l'=>mode&0xF000==0xA000, 'c'=>mode&0xF000==0x2000, 'b'=>mode&0xF000==0x6000, 'p'=>mode&0xF000==0x1000, 's'=>mode&0xF000==0xC000, _=>true },
            Predicate::Size(spec) => {
                let (cmp, n, unit) = parse_size_pred(spec);
                let sz_in = size / match unit { 'c'=>1, 'k'=>1024, 'M'=>1024*1024, 'G'=>1024*1024*1024, _=>512 };
                match cmp { '+'=>sz_in>n as i64, '-'=>sz_in<n as i64, _=>sz_in==n as i64 }
            }
            Predicate::Time(kind, n) => {
                let age_days = (mtime - now) / 86400;
                if *n > 0 { age_days <= *n } else if *n < 0 { -age_days < -n } else { age_days == 0 }
            }
            Predicate::Empty => {
                if is_dir { false } else { size == 0 }
            }
            Predicate::Not => { negate = true; continue; }
            Predicate::MaxDepth(_) | Predicate::MinDepth(_) | Predicate::Print(_) | Predicate::Ls | Predicate::Exec(_) | Predicate::Delete | Predicate::Prune => true,
            _ => true,
        };
        let final_result = if negate { !result } else { result };
        negate = false;
        if !final_result { return false; }
    }
    true
}

fn parse_size_pred(spec: &str) -> (char, u64, char) {
    let b = spec.as_bytes();
    if b.is_empty() { return ('=', 0, 'c'); }
    let (cmp, rest) = if b[0]==b'+' { ('+', &spec[1..]) } else if b[0]==b'-' { ('-', &spec[1..]) } else { ('=', spec) };
    let last = rest.chars().last().unwrap_or('c');
    let num_s = if last.is_alphabetic() { &rest[..rest.len()-1] } else { rest };
    let n = num_s.parse().unwrap_or(0);
    let unit = if last.is_alphabetic() { last } else { 'c' };
    (cmp, n, unit)
}

fn exec_find_cmd(parts: &[String], path: &str) {
    let replaced: Vec<String> = parts.iter().map(|p| if p=="{}" { path.to_string() } else { p.clone() }).collect();
    let pid = fork();
    if pid == 0 {
        let mut argv_strs: Vec<String> = replaced.iter().map(|s| { let mut x=s.clone(); x.push('\0'); x }).collect();
        argv_strs.push("\0".to_string());
        let argv: Vec<*const u8> = argv_strs.iter().map(|s| s.as_ptr() as *const u8).collect();
        let envp: [*const u8; 1] = [core::ptr::null()];
        let mut cmd = replaced[0].clone(); cmd.push('\0');
        execve(cmd.as_bytes(), &argv, &envp);
        exit(1);
    }
    if pid > 0 { let mut s=0i32; waitpid(pid as i32, &mut s, 0); }
}

fn glob_match(pat: &str, name: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let n: Vec<char> = name.chars().collect();
    glob_inner(&p, &n)
}
fn glob_inner(pat: &[char], name: &[char]) -> bool {
    if pat.is_empty() { return name.is_empty(); }
    match pat[0] { '*' => (0..=name.len()).any(|i| glob_inner(&pat[1..], &name[i..])), '?' => !name.is_empty() && glob_inner(&pat[1..], &name[1..]), c => !name.is_empty() && c==name[0] && glob_inner(&pat[1..], &name[1..]) }
}

fn format_mode_find(mode: u32) -> String {
    let ft = match mode&0xF000 { 0x8000=>'-', 0x4000=>'d', 0xA000=>'l', _=>'?' };
    let r = |s: u32, b: u32| -> char { if mode>>s&b!=0 { match b{4=>'r',2=>'w',_=>'x'} } else { '-' } };
    alloc::format!("{}{}{}{}{}{}{}{}{}{}", ft, r(6,4),r(6,2),r(6,1), r(3,4),r(3,2),r(3,1), r(0,4),r(0,2),r(0,1))
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
