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
    let mut preserve=false; let mut verbose=false; let mut no_clobber=false;
    let mut dereference=true; let mut update=false; let mut archive=false;
    let mut files: Vec<String>=Vec::new();
    let mut i=1;
    while i < args.len() {
        match args[i].as_str() {
            "-r"|"-R"|"--recursive" => recursive=true,
            "-f"|"--force" => force=true,
            "-i"|"--interactive" => interactive=true,
            "-p"|"--preserve" => preserve=true,
            "-v"|"--verbose" => verbose=true,
            "-n"|"--no-clobber" => no_clobber=true,
            "-P"|"--no-dereference" => dereference=false,
            "-L"|"--dereference" => dereference=true,
            "-u"|"--update" => update=true,
            "-a"|"--archive" => { archive=true; recursive=true; preserve=true; dereference=false; }
            "--" => { files.extend(args[i+1..].iter().cloned()); break; }
            s if s.starts_with('-') && !s.starts_with("--") => {
                for c in s[1..].chars() { match c { 'r'|'R'=>recursive=true,'f'=>force=true,'i'=>interactive=true,'p'=>preserve=true,'v'=>verbose=true,'n'=>no_clobber=true,'u'=>update=true,'a'=>{archive=true;recursive=true;preserve=true;dereference=false;}, _=>{} } }
            }
            _ => files.push(args[i].clone()),
        }
        i+=1;
    }
    if files.len() < 2 { write_err("cp: missing file operand\n"); exit(1); }
    let dst = files.last().unwrap().clone();
    let srcs = &files[..files.len()-1];

    // Check if dst is directory
    let dst_is_dir = { let mut p=dst.clone(); p.push('\0'); let mut st=[0u64;22]; (unsafe{syscall::syscall2(4,p.as_ptr() as u64,st.as_mut_ptr() as u64)})==0 && (st[2]>>32)&0xF000==0x4000 };

    for src in srcs {
        let dest = if dst_is_dir {
            let base=src.rsplit('/').next().unwrap_or(src); alloc::format!("{}/{}", dst, base)
        } else { dst.clone() };
        if verbose { write_str(&alloc::format!("'{}' -> '{}'\n", src, dest)); }
        if let Err(e) = do_copy(src, &dest, recursive, force, no_clobber, interactive, preserve, dereference, update) {
            write_err(&alloc::format!("cp: {}: {}\n", src, e));
        }
    }
    exit(0)
}

fn do_copy(src: &str, dst: &str, recursive: bool, force: bool, no_clobber: bool, interactive: bool, preserve: bool, deref: bool, update: bool) -> Result<(), &'static str> {
    let mut src_st=[0u64;22]; let mut sp=src.to_string(); sp.push('\0');
    if unsafe{syscall::syscall2(4,sp.as_ptr() as u64,src_st.as_mut_ptr() as u64)} < 0 { return Err("No such file"); }
    let src_mode = (src_st[2]>>32) as u32;
    let src_size = src_st[7] as u64;
    let src_mtime = src_st[11] as i64;
    let is_dir = src_mode&0xF000==0x4000;
    let is_link = src_mode&0xF000==0xA000;

    if is_link && !deref {
        let mut lbuf=[0u8;1024]; let n=unsafe{syscall::syscall3(89,sp.as_ptr() as u64,lbuf.as_mut_ptr() as u64,1024)};
        if n<0 { return Err("readlink failed"); }
        let target=String::from_utf8_lossy(&lbuf[..n as usize]).to_string();
        let mut dp=dst.to_string(); dp.push('\0'); let mut target_p=target.clone(); target_p.push('\0');
        unsafe{syscall::syscall2(88,target_p.as_ptr() as u64,dp.as_ptr() as u64)};
        return Ok(());
    }

    if is_dir {
        if !recursive { return Err("omitting directory"); }
        let mut dp=dst.to_string(); dp.push('\0');
        unsafe{syscall::syscall2(83,dp.as_ptr() as u64,src_mode as u64&0o777)};
        // Recurse
        let fd=open(sp.as_bytes(),0o200000,0); if fd<0{return Err("cannot open dir");}
        let mut buf=alloc::vec![0u8;32768]; let mut entries=Vec::new();
        loop{let n=getdents64(fd as i32,&mut buf);if n<=0{break;}
            let mut off=0; while off<n as usize{
                let reclen=u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
                let name=&buf[off+19..]; let nlen=name.iter().position(|&b|b==0).unwrap_or(0);
                let name_s=String::from_utf8_lossy(&name[..nlen]).to_string();
                if name_s!="."&&name_s!=".." {entries.push(name_s);}
                if reclen==0{break;} off+=reclen;
            }
        }
        close(fd as i32);
        for e in entries {
            let sub_src=alloc::format!("{}/{}", src, e);
            let sub_dst=alloc::format!("{}/{}", dst, e);
            let _ = do_copy(&sub_src, &sub_dst, recursive, force, no_clobber, interactive, preserve, deref, update);
        }
        return Ok(());
    }

    // Check dst exists
    let mut dst_st=[0u64;22]; let mut dp=dst.to_string(); dp.push('\0');
    let dst_exists = (unsafe{syscall::syscall2(4,dp.as_ptr() as u64,dst_st.as_mut_ptr() as u64)})==0;
    if dst_exists {
        if no_clobber { return Ok(()); }
        if update && src_mtime <= dst_st[11] as i64 { return Ok(()); }
        if interactive { write_err(&alloc::format!("cp: overwrite '{}'? ", dst)); let mut b=[0u8;4]; read(STDIN,&mut b); if b[0]!=b'y'&&b[0]!=b'Y'{return Ok(());} }
    }

    let src_fd=open(sp.as_bytes(),O_RDONLY,0); if src_fd<0{return Err("cannot open");}
    let dst_fd=open(dp.as_bytes(),O_WRONLY|O_CREAT|O_TRUNC,src_mode&0o777); if dst_fd<0{close(src_fd as i32);return Err("cannot create");}
    let mut buf=[0u8;65536];
    loop{let n=read(src_fd as i32,&mut buf); if n<=0{break;} let mut off=0; while off<n as usize{let w=write(dst_fd as i32,&buf[off..n as usize]);if w<=0{break;}off+=w as usize;}}
    close(src_fd as i32); close(dst_fd as i32);
    Ok(())
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
