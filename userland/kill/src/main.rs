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
    let mut sig: i32 = 15; // SIGTERM default
    let mut list_mode = false;
    let mut table_mode = false;
    let mut pids: Vec<i32> = Vec::new();
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "-l"|"--list"    => list_mode=true,
            "-L"|"--table"   => table_mode=true,
            "--help"         => { w("Usage: kill [-s SIGNAL | -SIGNAL] PID...\n"); exit(0); }
            "--version"      => { w("kill (Qunix) 1.0\n"); exit(0); }
            "-s"|"--signal"  => { i+=1; sig=name_to_sig(a.get(i).map(|s|s.as_str()).unwrap_or("15")); }
            "--"             => { i+=1; break; }
            s if s.starts_with('-') && s.len()>1 => {
                let n=&s[1..];
                if let Ok(num)=n.parse::<i32>() { sig=num; }
                else { sig=name_to_sig(n); }
            }
            _ => break,
        }
        i+=1;
    }
    while i < a.len() {
        match a[i].trim_start_matches('%').parse::<i32>() {
            Ok(p) => pids.push(p),
            Err(_) => { e(&alloc::format!("kill: invalid pid: '{}'\n", a[i])); exit(1); }
        }
        i+=1;
    }
    if list_mode || table_mode {
        const SIGS: &[&str]=&["HUP","INT","QUIT","ILL","TRAP","ABRT","BUS","FPE",
            "KILL","USR1","SEGV","USR2","PIPE","ALRM","TERM","STKFLT","CHLD",
            "CONT","STOP","TSTP","TTIN","TTOU","URG","XCPU","XFSZ","VTALRM",
            "PROF","WINCH","IO","PWR","SYS"];
        if table_mode {
            for (n,s) in SIGS.iter().enumerate() { w(&alloc::format!("{:2}) SIG{:<10}", n+1, s)); if (n+1)%4==0{w("\n");} }
            w("\n");
        } else {
            for (n,s) in SIGS.iter().enumerate() { w(&alloc::format!("{}) SIG{}\n", n+1, s)); }
        }
        exit(0);
    }
    if pids.is_empty() { e("kill: no process ID specified\n"); exit(1); }
    let mut status = 0i32;
    for pid in &pids {
        let r = kill(*pid, sig);
        if r < 0 {
            e(&alloc::format!("kill: ({}) - No such process\n", pid));
            status = 1;
        }
    }
    exit(status)
}

fn name_to_sig(name: &str) -> i32 {
    let upper = name.trim_start_matches("SIG").to_uppercase();
    match upper.as_str() {
        "HUP"|"1"=>1,"INT"|"2"=>2,"QUIT"|"3"=>3,"ILL"|"4"=>4,"TRAP"|"5"=>5,
        "ABRT"|"6"=>6,"BUS"|"7"=>7,"FPE"|"8"=>8,"KILL"|"9"=>9,"USR1"|"10"=>10,
        "SEGV"|"11"=>11,"USR2"|"12"=>12,"PIPE"|"13"=>13,"ALRM"|"14"=>14,
        "TERM"|"15"=>15,"CHLD"|"17"=>17,"CONT"|"18"=>18,"STOP"|"19"=>19,
        "TSTP"|"20"=>20,"WINCH"|"28"=>28,
        n => n.parse().unwrap_or(15)
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
