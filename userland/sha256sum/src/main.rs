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


fn av(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i); let mut n=0; while *p.add(n)!=0{n+=1;}
        String::from_utf8_lossy(core::slice::from_raw_parts(p,n)).to_string()
    }).collect()
}
fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }
fn rdall(fd: i32) -> String {
    let mut d=alloc::vec![0u8;1<<20]; let mut t=0;
    loop{if t>=d.len(){d.resize(d.len()*2,0);} let n=read(fd,&mut d[t..]); if n<=0{break;} t+=n as usize;}
    String::from_utf8_lossy(&d[..t]).to_string()
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let a = av(argc, argv);
    let mut check=false; let mut files: Vec<String>=Vec::new(); let mut i=1;
    while i<a.len(){match a[i].as_str(){"-c"|"--check"=>check=true,_=>files.push(a[i].clone())}i+=1;}
    let sha256=|data:&[u8]|->String{ sha256_digest(data) };
    let process=|fd:i32,name:&str|{
        let mut data=Vec::new(); let mut buf=[0u8;65536];
        loop{let n=read(fd,&mut buf);if n<=0{break;}data.extend_from_slice(&buf[..n as usize]);}
        let hash=sha256(&data);
        wstr(&alloc::format!("{}  {}\n",hash,name));
    };
    if files.is_empty(){process(STDIN,"-");}
    else{for f in &files{if f=="-"{process(STDIN,"-");}else{
        let mut p=f.clone();p.push('\0');let fd=open(p.as_bytes(),O_RDONLY,0);if fd<0{werr(&alloc::format!("sha256sum: {}: No such file\n",f));continue;}
        process(fd as i32,f);close(fd as i32);
    }}}
    exit(0)
}
fn sha256_digest(data: &[u8]) -> String {
    // SHA-256 implementation
    let mut h:[u32;8]=[0x6a09e667,0xbb67ae85,0x3c6ef372,0xa54ff53a,0x510e527f,0x9b05688c,0x1f83d9ab,0x5be0cd19];
    let k:[u32;64]=[0x428a2f98,0x71374491,0xb5c0fbcf,0xe9b5dba5,0x3956c25b,0x59f111f1,0x923f82a4,0xab1c5ed5,0xd807aa98,0x12835b01,0x243185be,0x550c7dc3,0x72be5d74,0x80deb1fe,0x9bdc06a7,0xc19bf174,0xe49b69c1,0xefbe4786,0x0fc19dc6,0x240ca1cc,0x2de92c6f,0x4a7484aa,0x5cb0a9dc,0x76f988da,0x983e5152,0xa831c66d,0xb00327c8,0xbf597fc7,0xc6e00bf3,0xd5a79147,0x06ca6351,0x14292967,0x27b70a85,0x2e1b2138,0x4d2c6dfc,0x53380d13,0x650a7354,0x766a0abb,0x81c2c92e,0x92722c85,0xa2bfe8a1,0xa81a664b,0xc24b8b70,0xc76c51a3,0xd192e819,0xd6990624,0xf40e3585,0x106aa070,0x19a4c116,0x1e376c08,0x2748774c,0x34b0bcb5,0x391c0cb3,0x4ed8aa4a,0x5b9cca4f,0x682e6ff3,0x748f82ee,0x78a5636f,0x84c87814,0x8cc70208,0x90befffa,0xa4506ceb,0xbef9a3f7,0xc67178f2];
    let rotr=|x:u32,n:u32|x.rotate_right(n);
    let mut msg=data.to_vec();
    let bit_len=(data.len() as u64)*8;
    msg.push(0x80);
    while msg.len()%64!=56{msg.push(0);}
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for block in msg.chunks(64){
        let mut w=[0u32;64];
        for i in 0..16{w[i]=u32::from_be_bytes(block[i*4..i*4+4].try_into().unwrap_or([0;4]));}
        for i in 16..64{let s0=rotr(w[i-15],7)^rotr(w[i-15],18)^(w[i-15]>>3);let s1=rotr(w[i-2],17)^rotr(w[i-2],19)^(w[i-2]>>10);w[i]=w[i-16].wrapping_add(s0).wrapping_add(w[i-7]).wrapping_add(s1);}
        let[mut a,mut b,mut c,mut d,mut e,mut f,mut g,mut hh]=[h[0],h[1],h[2],h[3],h[4],h[5],h[6],h[7]];
        for i in 0..64{
            let s1=rotr(e,6)^rotr(e,11)^rotr(e,25);let ch=(e&f)^((!e)&g);let t1=hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(k[i]).wrapping_add(w[i]);
            let s0=rotr(a,2)^rotr(a,13)^rotr(a,22);let maj=(a&b)^(a&c)^(b&c);let t2=s0.wrapping_add(maj);
            hh=g;g=f;f=e;e=d.wrapping_add(t1);d=c;c=b;b=a;a=t1.wrapping_add(t2);
        }
        h[0]=h[0].wrapping_add(a);h[1]=h[1].wrapping_add(b);h[2]=h[2].wrapping_add(c);h[3]=h[3].wrapping_add(d);
        h[4]=h[4].wrapping_add(e);h[5]=h[5].wrapping_add(f);h[6]=h[6].wrapping_add(g);h[7]=h[7].wrapping_add(hh);
    }
    h.iter().map(|x|alloc::format!("{:08x}",x)).collect()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
