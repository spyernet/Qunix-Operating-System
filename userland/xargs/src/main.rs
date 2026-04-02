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
    let mut cmd: Vec<String> = Vec::new();
    let mut max_args = 0usize;
    let mut replace_str: Option<String> = None;
    let mut null_delim = false;
    let mut interactive = false;
    let mut verbose = false;
    let mut no_run_if_empty = false;
    let mut _max_procs = 1usize;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-0" | "--null" => null_delim = true,
            "-n" | "--max-args" => { i+=1; max_args=args.get(i).and_then(|s|s.parse().ok()).unwrap_or(0); }
            "-I" | "--replace" => { i+=1; replace_str=args.get(i).cloned(); }
            "-i" => replace_str=Some("{}".to_string()),
            "-p" | "--interactive" => interactive=true,
            "-t" | "--verbose" => verbose=true,
            "-r" | "--no-run-if-empty" => no_run_if_empty=true,
            "-P" | "--max-procs" => { i+=1; _max_procs=args.get(i).and_then(|s|s.parse().ok()).unwrap_or(1); }
            "--" => { i+=1; cmd.extend(args[i..].iter().cloned()); break; }
            _ => { cmd.extend(args[i..].iter().cloned()); break; }
        }
        i+=1;
    }
    if cmd.is_empty() { cmd.push("echo".to_string()); }

    // Read stdin items
    let mut data = alloc::vec![0u8; 1<<20]; let mut tot=0;
    loop { let n=read(STDIN,&mut data[tot..]); if n<=0{break;} tot+=n as usize; if tot>=data.len(){data.resize(data.len()*2,0);} }
    let text = String::from_utf8_lossy(&data[..tot]).to_string();

    let items: Vec<String> = if null_delim {
        text.split('\0').filter(|s|!s.is_empty()).map(|s|s.to_string()).collect()
    } else {
        text.split_whitespace().map(|s|s.to_string()).collect()
    };

    if no_run_if_empty && items.is_empty() { exit(0); }

    let chunk_size = if max_args > 0 { max_args } else { items.len().max(1) };
    let mut i2 = 0;
    while i2 < items.len() || (items.is_empty() && i2 == 0) {
        let chunk: Vec<String> = items[i2..(i2+chunk_size).min(items.len())].to_vec();
        if items.is_empty() && i2 > 0 { break; }
        let run_cmd: Vec<String> = if let Some(ref rep) = replace_str {
            let item = chunk.first().cloned().unwrap_or_default();
            cmd.iter().map(|c| c.replace(rep, &item)).collect()
        } else {
            let mut v = cmd.clone(); v.extend(chunk.iter().cloned()); v
        };
        if verbose || interactive {
            write_err(&run_cmd.join(" ")); write_err("\n");
            if interactive {
                write_err("?");
                let mut b=[0u8;4]; let n=read(STDIN,&mut b);
                if n>0 && b[0]!=b'y' && b[0]!=b'Y' { i2+=chunk_size; continue; }
            }
        }
        let pid = fork();
        if pid==0 {
            let mut argv_strs: Vec<String> = run_cmd.iter().map(|s|{let mut x=s.clone();x.push('\0');x}).collect();
            argv_strs.push("\0".to_string());
            let argv: Vec<*const u8> = argv_strs.iter().map(|s|s.as_ptr() as *const u8).collect();
            let envp:[*const u8;1]=[core::ptr::null()];
            let mut c=run_cmd[0].clone(); c.push('\0');
            execve(c.as_bytes(),&argv,&envp);
            exit(127);
        }
        if pid>0 { let mut s=0i32; waitpid(pid as i32,&mut s,0); }
        if items.is_empty() { break; }
        i2+=chunk_size;
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
