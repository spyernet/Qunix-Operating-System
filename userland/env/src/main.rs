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
pub extern "C" fn _start(argc: u64, argv: *const *const u8, envp: *const *const u8) -> ! {
    let a = parse_argv(argc, argv);
    let mut null_term=false; let mut ignore_env=false;
    let mut unset: Vec<String>=Vec::new();
    let mut set_vars: Vec<(String,String)>=Vec::new();
    let mut cmd_start = 1usize;
    let mut i = 1;
    while i < a.len() {
        match a[i].as_str() {
            "-0"|"--null"               => null_term=true,
            "-i"|"--ignore-environment" => ignore_env=true,
            "-u"|"--unset"              => { i+=1; unset.push(a.get(i).cloned().unwrap_or_default()); }
            "--help"                    => { w("Usage: env [OPTION]... [-] [NAME=VALUE]... [COMMAND [ARG]...]\n"); exit(0); }
            "--version"                 => { w("env (Qunix) 1.0\n"); exit(0); }
            "--"                        => { cmd_start=i+1; break; }
            "-"                         => { ignore_env=true; cmd_start=i+1; break; }
            s if s.contains('=')        => { if let Some(eq)=s.find('=') { set_vars.push((s[..eq].to_string(),s[eq+1..].to_string())); } }
            s if s.starts_with("-u")    => unset.push(s[2..].to_string()),
            _                           => { cmd_start=i; break; }
        }
        i+=1;
    }
    // Build environment map
    let mut env_map: alloc::collections::BTreeMap<String,String>=alloc::collections::BTreeMap::new();
    if !ignore_env {
        let mut ep=envp;
        loop { let p=unsafe{*ep}; if p.is_null(){break;}
            let s=cstr(p); if let Some(eq)=s.find('='){env_map.insert(s[..eq].to_string(),s[eq+1..].to_string());}
            ep=unsafe{ep.add(1)};
        }
    }
    for k in &unset { env_map.remove(k); }
    for (k,v) in &set_vars { env_map.insert(k.clone(),v.clone()); }

    let sep: u8 = if null_term { 0 } else { b'\n' };

    if cmd_start >= a.len() {
        for (k,v) in &env_map { w(&alloc::format!("{}={}", k, v)); write(STDOUT, &[sep]); }
        exit(0);
    }

    // Execute command with modified environment
    let env_strs: Vec<String>=env_map.iter().map(|(k,v)|alloc::format!("{}={}\0",k,v)).collect();
    let env_ptrs: Vec<*const u8>=env_strs.iter().map(|s|s.as_ptr() as *const u8).chain(core::iter::once(core::ptr::null())).collect();
    let cmd_args=&a[cmd_start..];
    let argv_strs: Vec<String>=cmd_args.iter().map(|s|{let mut x=s.clone();x.push('\0');x}).collect();
    let argv_ptrs: Vec<*const u8>=argv_strs.iter().map(|s|s.as_ptr() as *const u8).chain(core::iter::once(core::ptr::null())).collect();
    let mut cmd=cmd_args[0].clone(); cmd.push('\0');
    execve(cmd.as_bytes(),&argv_ptrs,&env_ptrs);
    e(&alloc::format!("env: {}: No such file or directory\n",cmd_args[0]));
    exit(127)
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"panic\n");
    exit(1)
}
