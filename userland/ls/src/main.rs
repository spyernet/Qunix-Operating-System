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
fn cstr_str(p: *const u8) -> String {
    unsafe {
        let mut len = 0; while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }
}

use core::fmt::Write as FmtWrite;

#[repr(C)] struct StatBuf {
    st_dev:u64, st_ino:u64, st_nlink:u64, st_mode:u32, st_uid:u32, st_gid:u32,
    _pad:u32, st_rdev:u64, st_size:i64, st_blksize:i64, st_blocks:i64,
    st_atime:i64, _atime_ns:i64, st_mtime:i64, _mtime_ns:i64,
    st_ctime:i64, _ctime_ns:i64, _unused:[i64;3],
}

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let args = args_from_argv(argc, argv);
    let mut long_fmt = false;
    let mut all = false;
    let mut almost_all = false;
    let mut human = false;
    let mut inode = false;
    let mut one_per_line = false;
    let mut classify = false;
    let mut colorize = true;
    let mut reverse = false;
    let mut sort_time = false;
    let mut sort_size = false;
    let mut no_sort = false;
    let mut recursive = false;
    let mut dir_itself = false;
    let mut numeric_uid = false;
    let mut paths: Vec<String> = Vec::new();

    let mut i = 1;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "-l" => long_fmt = true,
            "-a" => all = true,
            "-A" => almost_all = true,
            "-h" => human = true,
            "-i" => inode = true,
            "-1" => one_per_line = true,
            "-F" => classify = true,
            "--color=never" | "--no-color" => colorize = false,
            "--color=always" | "--color=auto" | "--color" => colorize = true,
            "-r" => reverse = true,
            "-t" => sort_time = true,
            "-S" => sort_size = true,
            "-U" => no_sort = true,
            "-R" => recursive = true,
            "-d" => dir_itself = true,
            "-n" => numeric_uid = true,
            "-C" | "-x" => one_per_line = false,
            "--" => { paths.extend(args[i+1..].iter().cloned()); break; }
            s if s.starts_with("--") => {}
            s if s.starts_with('-') => {
                for c in s[1..].chars() {
                    match c {
                        'l' => long_fmt = true,
                        'a' => all = true,
                        'A' => almost_all = true,
                        'h' => human = true,
                        'i' => inode = true,
                        '1' => one_per_line = true,
                        'F' => classify = true,
                        'r' => reverse = true,
                        't' => sort_time = true,
                        'S' => sort_size = true,
                        'U' => no_sort = true,
                        'R' => recursive = true,
                        'd' => dir_itself = true,
                        'n' => numeric_uid = true,
                        _ => {}
                    }
                }
            }
            _ => paths.push(a.clone()),
        }
        i += 1;
    }

    if paths.is_empty() { paths.push(".".to_string()); }

    let multiple = paths.len() > 1 || (recursive && paths.len() >= 1);
    for (pi, path) in paths.iter().enumerate() {
        if multiple { write_str(&alloc::format!("{}:\n", path)); }
        list_path(path, long_fmt, all || almost_all, human, inode, one_per_line || long_fmt,
                  classify, colorize, reverse, sort_time, sort_size, no_sort, recursive, dir_itself, numeric_uid);
        if multiple && pi + 1 < paths.len() { write(STDOUT, b"\n"); }
    }
    exit(0)
}

fn list_path(path: &str, long: bool, all: bool, human: bool, inode: bool,
             one: bool, classify: bool, color: bool, rev: bool,
             sort_t: bool, sort_s: bool, no_sort: bool, recur: bool, dir_itself: bool, numeric: bool) {
    let mut st = StatBuf { st_dev:0, st_ino:0, st_nlink:0, st_mode:0, st_uid:0, st_gid:0,
        _pad:0, st_rdev:0, st_size:0, st_blksize:0, st_blocks:0,
        st_atime:0, _atime_ns:0, st_mtime:0, _mtime_ns:0, st_ctime:0, _ctime_ns:0, _unused:[0;3] };
    let mut p = path.to_string(); p.push('\0');
    unsafe { syscall::syscall2(4, p.as_ptr() as u64, &mut st as *mut _ as u64) };

    let is_dir = st.st_mode & 0xF000 == 0x4000;
    if !is_dir || dir_itself {
        print_entry(path, &st, long, human, inode, classify, color, numeric);
        if !one { write(STDOUT, b"\n"); }
        return;
    }

    let fd = open(p.as_bytes(), 0o200000, 0);
    if fd < 0 { write_err(&alloc::format!("ls: cannot open directory '{}'\n", path)); return; }

    let mut entries: Vec<(String, StatBuf)> = Vec::new();
    let mut buf = alloc::vec![0u8; 32768];
    loop {
        let n = getdents64(fd as i32, &mut buf);
        if n <= 0 { break; }
        let mut off = 0;
        while off < n as usize {
            let reclen = u16::from_le_bytes(buf[off+16..off+18].try_into().unwrap_or([0;2])) as usize;
            let name = &buf[off+19..];
            let nlen = name.iter().position(|&b| b==0).unwrap_or(0);
            let name_s = String::from_utf8_lossy(&name[..nlen]).to_string();
            if !all && name_s.starts_with('.') { if reclen == 0 { break; } off += reclen; continue; }
            let full = if path == "." { name_s.clone() } else { alloc::format!("{}/{}", path, name_s) };
            let mut est = StatBuf { st_dev:0, st_ino:0, st_nlink:0, st_mode:0, st_uid:0, st_gid:0,
                _pad:0, st_rdev:0, st_size:0, st_blksize:0, st_blocks:0,
                st_atime:0, _atime_ns:0, st_mtime:0, _mtime_ns:0, st_ctime:0, _ctime_ns:0, _unused:[0;3] };
            let mut fp = full.clone(); fp.push('\0');
            unsafe { syscall::syscall2(4, fp.as_ptr() as u64, &mut est as *mut _ as u64) };
            entries.push((name_s, est));
            if reclen == 0 { break; }
            off += reclen;
        }
    }
    close(fd as i32);

    if !no_sort {
        entries.sort_by(|a, b| {
            if sort_t { b.1.st_mtime.cmp(&a.1.st_mtime) }
            else if sort_s { b.1.st_size.cmp(&a.1.st_size) }
            else { a.0.cmp(&b.0) }
        });
    }
    if rev { entries.reverse(); }

    if long {
        let total_blocks: i64 = entries.iter().map(|e| (e.1.st_blocks + 7) / 8).sum();
        write_str(&alloc::format!("total {}\n", total_blocks));
    }

    let mut sub_dirs: Vec<String> = Vec::new();
    let col_width = entries.iter().map(|e| e.0.len()).max().unwrap_or(8) + 2;
    let term_cols = 80usize;
    let cols = if one || long { 1 } else { (term_cols / col_width).max(1) };
    let rows = (entries.len() + cols - 1) / cols;

    for row in 0..rows {
        for col in 0..cols {
            let idx = if one || long { row } else { row + col * rows };
            if idx >= entries.len() { break; }
            let (name, est) = &entries[idx];
            if long {
                print_long(path, name, est, human, inode, color, numeric);
            } else {
                if inode { write_str(&alloc::format!("{:6} ", est.st_ino)); }
                let suffix = if classify { file_suffix(est.st_mode) } else { "" };
                let colored = if color { colorize_name(name, est.st_mode) } else { name.clone() };
                write_str(&alloc::format!("{}{}", colored, suffix));
                if col + 1 < cols { for _ in 0..(col_width.saturating_sub(name.len() + suffix.len())) { write(STDOUT, b" "); } }
            }
        }
        if !long || entries.len() > row { write(STDOUT, b"\n"); }
    }

    if recur {
        for (name, est) in &entries {
            if est.st_mode & 0xF000 == 0x4000 && name != "." && name != ".." {
                let sub = if path == "." { name.clone() } else { alloc::format!("{}/{}", path, name) };
                write_str(&alloc::format!("\n{}:\n", sub));
                list_path(&sub, long, all, human, inode, one, classify, color, rev, sort_t, sort_s, no_sort, true, false, numeric);
                sub_dirs.push(sub);
            }
        }
    }
}

fn print_long(dir: &str, name: &str, st: &StatBuf, human: bool, inode: bool, color: bool, numeric: bool) {
    if inode { write_str(&alloc::format!("{:6} ", st.st_ino)); }
    let mode = format_mode(st.st_mode);
    let size_s = if human { human_size(st.st_size as u64) } else { alloc::format!("{:8}", st.st_size) };
    let time_s = format_time(st.st_mtime);
    let colored = if color { colorize_name(name, st.st_mode) } else { name.to_string() };
    let suffix = file_suffix(st.st_mode);
    // Symlink target
    let link_s = if st.st_mode & 0xF000 == 0xA000 {
        let full = if dir == "." { alloc::format!("{}\0", name) } else { alloc::format!("{}/{}\0", dir, name) };
        let mut lbuf = [0u8; 1024];
        let n = unsafe { syscall::syscall3(89, full.as_ptr() as u64, lbuf.as_mut_ptr() as u64, 1024) };
        if n > 0 { alloc::format!(" -> {}", String::from_utf8_lossy(&lbuf[..n as usize])) }
        else { String::new() }
    } else { String::new() };
    write_str(&alloc::format!("{} {:3} {:5} {:5} {} {} {}{}{}\n",
        mode, st.st_nlink, st.st_uid, st.st_gid, size_s, time_s, colored, suffix, link_s));
}

fn format_mode(mode: u32) -> String {
    let ft = match mode & 0xF000 {
        0x8000 => '-', 0x4000 => 'd', 0xA000 => 'l', 0x2000 => 'c',
        0x6000 => 'b', 0x1000 => 'p', 0xC000 => 's', _ => '?',
    };
    let bits = |sh: u32| -> [char; 3] {
        let r = if mode >> sh & 4 != 0 { 'r' } else { '-' };
        let w = if mode >> sh & 2 != 0 { 'w' } else { '-' };
        let x = if mode >> sh & 1 != 0 { 'x' } else { '-' };
        [r, w, x]
    };
    let [ur,uw,ux] = bits(6); let [gr,gw,gx] = bits(3); let [or,ow,ox] = bits(0);
    let ux = if mode & 0o4000 != 0 { if ux == 'x' { 's' } else { 'S' } } else { ux };
    let gx = if mode & 0o2000 != 0 { if gx == 'x' { 's' } else { 'S' } } else { gx };
    let ox = if mode & 0o1000 != 0 { if ox == 'x' { 't' } else { 'T' } } else { ox };
    alloc::format!("{}{}{}{}{}{}{}{}{}{}", ft, ur, uw, ux, gr, gw, gx, or, ow, ox)
}

fn format_time(ts: i64) -> String {
    // Simple time: Mon DD HH:MM or Mon DD  YYYY
    let s = ts % 86400; let d = ts / 86400;
    let year_base = 1970i64;
    // Very simplified: just show timestamp
    alloc::format!("Jan  1 {:02}:{:02}", (s/3600)%24, (s/60)%60)
}

fn human_size(n: u64) -> String {
    if n < 1024 { return alloc::format!("{:4}", n); }
    if n < 1024*1024 { return alloc::format!("{:.1}K", n as f64/1024.0); }
    if n < 1024*1024*1024 { return alloc::format!("{:.1}M", n as f64/1048576.0); }
    alloc::format!("{:.1}G", n as f64/1073741824.0)
}

fn file_suffix(mode: u32) -> &'static str {
    match mode & 0xF000 { 0x4000 => "/", 0xA000 => "@", 0xC000 => "=", 0x1000 => "|", _ => {
        if mode & 0o111 != 0 { "*" } else { "" }
    }}
}

fn colorize_name(name: &str, mode: u32) -> String {
    let color = match mode & 0xF000 {
        0x4000 => "\x1b[1;34m",  // dir: bold blue
        0xA000 => "\x1b[1;36m",  // symlink: bold cyan
        0x2000 | 0x6000 => "\x1b[1;33m", // device: bold yellow
        0x1000 => "\x1b[33m",    // pipe: yellow
        0xC000 => "\x1b[1;35m",  // socket: bold magenta
        0x8000 => {
            if mode & 0o111 != 0 { "\x1b[1;32m" } // exec: bold green
            else if name.ends_with(".tar") || name.ends_with(".gz") || name.ends_with(".zip")
                   || name.ends_with(".xz") || name.ends_with(".bz2") || name.ends_with(".zst") { "\x1b[1;31m" } // archive: bold red
            else if name.ends_with(".png") || name.ends_with(".jpg") || name.ends_with(".jpeg")
                   || name.ends_with(".gif") || name.ends_with(".svg") || name.ends_with(".webp") { "\x1b[35m" } // image: magenta
            else { "" }
        }
        _ => "",
    };
    if color.is_empty() { name.to_string() }
    else { alloc::format!("{}{}\x1b[0m", color, name) }
}

fn print_entry(name: &str, st: &StatBuf, long: bool, human: bool, inode: bool, classify: bool, color: bool, numeric: bool) {
    if inode { write_str(&alloc::format!("{:6} ", st.st_ino)); }
    let colored = if color { colorize_name(name, st.st_mode) } else { name.to_string() };
    let suffix = if classify { file_suffix(st.st_mode) } else { "" };
    if long { print_long(".", name, st, human, inode, color, numeric); }
    else { write_str(&alloc::format!("{}{}", colored, suffix)); }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { write(STDERR, b"panic\n"); exit(1) }
