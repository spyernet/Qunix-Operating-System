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
    let mut recursive=false; let mut force=false; let mut interactive=false;
    let mut verbose=false; let mut no_preserve_root=false; let mut files: Vec<String>=Vec::new();
    let mut i=1;
    while i<args.len() {
        match args[i].as_str() {
            "-r"|"-R"|"--recursive"=>recursive=true,"-f"|"--force"=>force=true,
            "-i"|"--interactive"=>interactive=true,"-v"|"--verbose"=>verbose=true,
            "--no-preserve-root"=>no_preserve_root=true,"--preserve-root"=>{},"--"=>{i+=1;files.extend(args[i..].iter().cloned());break;}
            s if s.starts_with('-')&&!s.starts_with("--")=>{for c in s[1..].chars(){match c{'r'|'R'=>recursive=true,'f'=>force=true,'i'=>interactive=true,'v'=>verbose=true,_=>{}}}}
            _=>files.push(args[i].clone()),
        }
        i+=1;
    }
    let mut status=0i32;
    for f in &files {
        if f=="/" && !no_preserve_root{write_err("rm: refusing to remove '/'\n");status=1;continue;}
        if interactive{write_err(&alloc::format!("rm: remove '{}'? ",f));let mut b=[0u8;4];read(STDIN,&mut b);if b[0]!=b'y'&&b[0]!=b'Y'{continue;}}
        if verbose{write_str(&alloc::format!("removed '{}'\n",f));}
        if let Err(e)=do_rm(f,recursive,force){
            if !force{write_err(&alloc::format!("rm: cannot remove '{}': {}\n",f,e));status=1;}
        }
    }
    exit(status)
}

fn do_rm(path: &str, recursive: bool, force: bool) -> Result<(), &'static str> {
    let mut p=path.to_string(); p.push('\0');
    let mut st=[0u64;22];
    let r=unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)};
    if r<0{if force{return Ok(());}return Err("No such file");}
    let mode=(st[2]>>32)as u32;
    let is_dir=mode&0xF000==0x4000;
    if is_dir {
        if !recursive{return Err("is a directory");}
        let fd=open(p.as_bytes(),0o200000,0);
        if fd>=0{
            let mut buf=alloc::vec![0u8;32768]; let mut entries=Vec::new();
            loop{let n=getdents64(fd as i32,&mut buf);if n<=0{break;}
                let mut off=0;while off<n as usize{
                    let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2]))as usize;
                    let name=&buf[off+19..];let nlen=name.iter().position(|&b|b==0).unwrap_or(0);
                    let name_s=String::from_utf8_lossy(&name[..nlen]).to_string();
                    if name_s!="."&&name_s!=".."{entries.push(name_s);}
                    if reclen==0{break;}off+=reclen;
                }
            }
            close(fd as i32);
            for e in entries{let sub=alloc::format!("{}/{}",path,e);let _=do_rm(&sub,recursive,force);}
        }
        if unsafe{syscall::syscall1(84,p.as_ptr() as u64)}<0{return Err("cannot remove dir");}
    } else {
        if unsafe{syscall::syscall1(87,p.as_ptr() as u64)}<0{return Err("cannot remove");}
    }
    Ok(())
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
