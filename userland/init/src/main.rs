#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;

const TIOCSCTTY: u64 = 0x540E;
const TIOCSPGRP: u64 = 0x5410;


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

fn attach_console(console: &[u8]) {
    let fd = open(console, O_RDWR | O_NOCTTY, 0);
    if fd < 0 { return; }

    let _ = ioctl(fd as i32, TIOCSCTTY, 0);
    let pgid = getpid() as u32;
    let _ = ioctl(fd as i32, TIOCSPGRP, &pgid as *const u32 as u64);

    dup2(fd as i32, STDIN);
    dup2(fd as i32, STDOUT);
    dup2(fd as i32, STDERR);
    if fd > 2 { close(fd as i32); }
}

#[no_mangle] #[link_section = ".text._start"]
pub extern "C" fn _start() -> ! {
    // PID 1 - init system
    wstr("Qunix init starting...\n");

    // Mount filesystems
    let mounts = [
        (b"proc\0".as_ptr() as u64, b"/proc\0".as_ptr() as u64, b"proc\0".as_ptr() as u64),
        (b"sysfs\0".as_ptr() as u64, b"/sys\0".as_ptr() as u64, b"sysfs\0".as_ptr() as u64),
        (b"devtmpfs\0".as_ptr() as u64, b"/dev\0".as_ptr() as u64, b"devtmpfs\0".as_ptr() as u64),
        (b"tmpfs\0".as_ptr() as u64, b"/tmp\0".as_ptr() as u64, b"tmpfs\0".as_ptr() as u64),
        (b"tmpfs\0".as_ptr() as u64, b"/run\0".as_ptr() as u64, b"tmpfs\0".as_ptr() as u64),
    ];
    for (src,dst,fstype) in &mounts {
        unsafe { syscall::syscall5(165, *src, *dst, *fstype, 0u64, 0u64) };
    }

    // Create essential device nodes
    let devs: [(&[u8], u32, u32); 6] = [
        (b"/dev/null\0".as_slice(), 0x0103u32, 0o666u32),
        (b"/dev/zero\0".as_slice(), 0x0105u32, 0o666u32),
        (b"/dev/tty\0".as_slice(), 0x0500u32, 0o666u32),
        (b"/dev/ptmx\0".as_slice(), 0x0502u32, 0o666u32),
        (b"/dev/random\0".as_slice(), 0x0108u32, 0o444u32),
        (b"/dev/urandom\0".as_slice(), 0x0109u32, 0o444u32),
    ];
    for (path, dev, mode) in &devs {
        unsafe { syscall::syscall3(133, path.as_ptr() as u64, (0x2000|mode) as u64, *dev as u64) };
    }

    // Set hostname
    unsafe { syscall::syscall2(170, b"qunix\0".as_ptr() as u64, 5u64) };

    // Source /etc/profile
    let profile = rdfile("/etc/profile");
    if !profile.is_empty() {
        wstr("Sourcing /etc/profile...\n");
    }

    // Start getty/shell on console
    wstr("Starting shell...\n");

    // Default consoles: prefer the graphical TTY if present, otherwise
    // fall back to the serial device used by headless setups.
    let consoles = [b"/dev/tty\0", b"/dev/serial\0"];
    let mut pids = [0i64; 2];
    for (idx, console) in consoles.iter().enumerate() {
        let pid = fork();
        if pid == 0 {
            wstr("init: child resumed after fork\n");
            // Create a new session so this child is not the controlling-terminal
            // leader of init's session. Without setsid() the child inherits
            // init's session and open(/dev/tty) would make it the controlling
            // terminal immediately — then tty_read() blocks on the first read
            // before qshell even starts.
            let _ = setsid();
            attach_console(*console);
            wstr("init: launching qshell on console\n");
            // Exec shell
            let shell_argv: [*const u8; 3] = [
                b"/bin/qsh\0".as_ptr(), b"-i\0".as_ptr(), core::ptr::null()
            ];
            let env_arr: [*const u8; 5] = [
                b"PATH=/bin:/sbin:/usr/bin:/usr/sbin:/usr/local/bin\0".as_ptr(),
                b"HOME=/root\0".as_ptr(), b"TERM=xterm-256color\0".as_ptr(),
                b"SHELL=/bin/qsh\0".as_ptr(), core::ptr::null()
            ];
            execve(b"/bin/qsh\0", &shell_argv, &env_arr);
            exit(1);
        }
        wstr("init: parent resumed after fork\n");
        pids[idx] = pid;
    }

    // Init main loop: reap zombies and restart died processes
    loop {
        let mut status = 0i32;
        let pid = unsafe { syscall::syscall3(61, (-1i64) as u64, &mut status as *mut _ as u64, 0u64) } as i32;
        if pid > 0 {
            // Restart shell if it dies
            for (idx, &sp) in pids.iter().enumerate() {
                if sp as i32 == pid {
                    let new_pid = fork();
                    if new_pid == 0 {
                        let _ = setsid();
                        attach_console(consoles[idx]);
                        wstr("init: restarting qshell on console\n");
                        let shell_argv: [*const u8; 2] = [b"/bin/qsh\0".as_ptr(), core::ptr::null()];
                        let env_arr: [*const u8; 2] = [b"PATH=/bin:/sbin:/usr/bin\0".as_ptr(), core::ptr::null()];
                        execve(b"/bin/qsh\0", &shell_argv, &env_arr);
                        exit(1);
                    }
                    pids[idx] = new_pid;
                    break;
                }
            }
        } else {
            // Sleep briefly to avoid spinning
            nanosleep_ms(1000);
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
