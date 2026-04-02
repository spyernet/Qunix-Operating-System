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
    let mut recursive=false; let mut verbose=false; let mut changes_only=false;
    let mut mode_str=String::new(); let mut files: Vec<String>=Vec::new();
    let mut i=1;
    while i<args.len() {
        match args[i].as_str() {
            "-R"|"--recursive"=>recursive=true,"-v"|"--verbose"=>verbose=true,
            "-c"|"--changes"=>changes_only=true,"--"=>{i+=1;break;}
            s if s.starts_with('-')&&!s.starts_with("--")=>{for c in s[1..].chars(){match c{'R'=>recursive=true,'v'=>verbose=true,'c'=>changes_only=true,_=>{}}}}
            _=>{if mode_str.is_empty(){mode_str=args[i].clone();}else{files.push(args[i].clone());}}
        }
        i+=1;
    }
    while i<args.len(){files.push(args[i].clone());i+=1;}
    if mode_str.is_empty()||files.is_empty(){write_err("chmod: missing operand\n");exit(1);}
    let mut status=0i32;
    for f in &files {
        if let Err(e)=do_chmod(f,&mode_str,recursive,verbose){write_err(&alloc::format!("chmod: {}: {}\n",f,e));status=1;}
    }
    exit(status)
}

fn do_chmod(path: &str, mode_s: &str, recursive: bool, verbose: bool) -> Result<(), &'static str> {
    let mut p=path.to_string();p.push('\0');
    let mut st=[0u64;22];
    if unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)}<0{return Err("No such file");}
    let cur_mode=(st[2]>>32) as u32;
    let new_mode=apply_mode(cur_mode, mode_s);
    if unsafe{syscall::syscall2(90,p.as_ptr() as u64,new_mode as u64)}<0{return Err("Operation not permitted");}
    if verbose{write_str(&alloc::format!("mode of '{}' changed to {:04o}\n",path,new_mode&0o777));}
    if recursive&&cur_mode&0xF000==0x4000 {
        let fd=open(p.as_bytes(),0o200000,0);
        if fd>=0{
            let mut buf=alloc::vec![0u8;32768]; let mut entries=Vec::new();
            loop{let n=getdents64(fd as i32,&mut buf);if n<=0{break;}
                let mut off=0;while off<n as usize{
                    let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2]))as usize;
                    let name=&buf[off+19..];let nlen=name.iter().position(|&b|b==0).unwrap_or(0);
                    let name_s=String::from_utf8_lossy(&name[..nlen]).to_string();
                    if name_s!="."&&name_s!=".."{ entries.push(name_s); }
                    if reclen==0{break;}off+=reclen;
                }
            }
            close(fd as i32);
            for e in entries{let sub=alloc::format!("{}/{}",path,e);let _=do_chmod(&sub,mode_s,recursive,verbose);}
        }
    }
    Ok(())
}

fn apply_mode(cur: u32, spec: &str) -> u32 {
    // Octal mode
    if spec.chars().all(|c| "01234567".contains(c)) {
        return (cur&0xF000)|u32::from_str_radix(spec,8).unwrap_or(cur&0o777);
    }
    // Symbolic mode: [ugoa][+-=][rwxXst],...
    let mut mode=cur&0o777;
    for part in spec.split(',') {
        let b=part.as_bytes(); if b.is_empty(){continue;}
        let mut who_end=0; while who_end<b.len()&&matches!(b[who_end],b'u'|b'g'|b'o'|b'a'){who_end+=1;}
        if who_end>=b.len(){continue;}
        let who=&b[..who_end]; let op=b[who_end]; let perms=&b[who_end+1..];
        let mut mask=0u32;
        for &p in perms{ mask|=match p{b'r'=>0o444,b'w'=>0o222,b'x'=>0o111,b'X'=>if cur&0o111!=0{0o111}else{0},b's'=>0o6000,b't'=>0o1000,_=>0}; }
        let apply_mask=if who.is_empty()||who.contains(&b'a'){mask}else{
            let mut m=0u32;
            for &w in who{m|=match w{b'u'=>(mask>>6&7)<<6,b'g'=>(mask>>3&7)<<3,b'o'=>mask&7,_=>0};}
            m
        };
        mode=match op{b'+'=>mode|apply_mask,b'-'=>mode&!apply_mask,b'='=>(mode&!0o777)|(apply_mask&0o777),_=>mode};
    }
    (cur&0xF000)|mode
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
