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


fn args(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i); let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }).collect()
}
fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> String {
    let mut d = alloc::vec![0u8; 1<<20]; let mut t = 0;
    loop { if t >= d.len() { d.resize(d.len()*2,0); } let n = read(fd, &mut d[t..]); if n<=0{break;} t+=n as usize; }
    String::from_utf8_lossy(&d[..t]).to_string()
}
fn rdfile(p: &str) -> String {
    let mut pa = p.to_string(); pa.push('\0');
    let fd = open(pa.as_bytes(), O_RDONLY, 0); if fd < 0 { return String::new(); }
    let s = rdall(fd as i32); close(fd as i32); s
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = args(argc, argv);
    let mut fmt=String::from("%a %b %e %H:%M:%S %Z %Y"); let mut utc=false; let mut i=1;
    while i<a.len() {
        match a[i].as_str() {
            "-u"|"--utc"|"--universal" => utc=true,
            "-d"|"--date" => { i+=1; }
            s if s.starts_with('+') => fmt=s[1..].to_string(),
            _ => {}
        }
        i+=1;
    }
    let mut ts=[0i64;2]; clock_gettime(0,&mut ts);
    let epoch=ts[0]; let ns=ts[1];
    let s=epoch%86400; let h=(s/3600)%24; let mi=(s/60)%60; let sc=s%60;
    let (yr,mo,dy,wd)=epoch_to_ymd(epoch);
    let months=["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
    let days  =["Sun","Mon","Tue","Wed","Thu","Fri","Sat"];
    let full_months=["January","February","March","April","May","June","July","August","September","October","November","December"];
    let full_days  =["Sunday","Monday","Tuesday","Wednesday","Thursday","Friday","Saturday"];
    let out = fmt
        .replace("%Y",&alloc::format!("{:04}",yr)).replace("%y",&alloc::format!("{:02}",yr%100))
        .replace("%m",&alloc::format!("{:02}",mo)).replace("%d",&alloc::format!("{:02}",dy))
        .replace("%e",&alloc::format!("{:2}",dy)).replace("%j",&alloc::format!("{:03}",doy(yr,mo,dy)))
        .replace("%H",&alloc::format!("{:02}",h)).replace("%M",&alloc::format!("{:02}",mi))
        .replace("%S",&alloc::format!("{:02}",sc)).replace("%N",&alloc::format!("{:09}",ns))
        .replace("%s",&epoch.to_string())
        .replace("%A",full_days.get(wd as usize%7).unwrap_or(&"Mon"))
        .replace("%a",&full_days.get(wd as usize%7).unwrap_or(&"Mon")[..3])
        .replace("%B",full_months.get((mo-1) as usize).unwrap_or(&"Jan"))
        .replace("%b",&months.get((mo-1) as usize).unwrap_or(&"Jan").to_string())
        .replace("%Z",if utc{"UTC"}else{"UTC"}).replace("%z","+0000")
        .replace("%n","\n").replace("%t","\t").replace("%%","%")
        .replace("%D",&alloc::format!("{:02}/{:02}/{:02}",mo,dy,yr%100))
        .replace("%T",&alloc::format!("{:02}:{:02}:{:02}",h,mi,sc))
        .replace("%F",&alloc::format!("{:04}-{:02}-{:02}",yr,mo,dy))
        .replace("%R",&alloc::format!("{:02}:{:02}",h,mi))
        .replace("%I",&alloc::format!("{:02}",if h==0{12}else if h>12{h-12}else{h}))
        .replace("%p",if h<12{"AM"}else{"PM"})
        .replace("%u",&alloc::format!("{}",if wd==0{7}else{wd}))
        .replace("%w",&wd.to_string());
    wstr(&out); wstr("\n"); exit(0)
}
fn epoch_to_ymd(e: i64) -> (i64,i64,i64,i64) {
    let wday=(e/86400+4)%7;
    let mut d=e/86400; let mut y=1970i64;
    loop{let dy=if is_leap(y){366}else{365};if d<dy{break;}d-=dy;y+=1;}
    let ml=[31i64,if is_leap(y){29}else{28},31,30,31,30,31,31,30,31,30,31];
    let mut m=1i64; for dm in &ml{if d<*dm{break;}d-=dm;m+=1;}
    (y,m,d+1,wday)
}
fn is_leap(y:i64)->bool{y%400==0||(y%4==0&&y%100!=0)}
fn doy(y:i64,m:i64,d:i64)->i64{
    let ml=[31i64,if is_leap(y){29}else{28},31,30,31,30,31,31,30,31,30,31];
    ml[..(m-1)as usize].iter().sum::<i64>()+d
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
