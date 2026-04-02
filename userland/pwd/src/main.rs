#![no_std]
#![no_main]
extern crate alloc;
use libsys::*;

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let logical = unsafe {
        (1..argc as usize).any(|i| {
            let p = *argv.add(i);
            let mut l = 0; while *p.add(l) != 0 { l += 1; }
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(p, l)) == "-P"
        })
    };
    // Always report logical cwd (from getcwd syscall which tracks it)
    let mut buf = [0u8; 4096];
    let n = getcwd(&mut buf);
    if n > 0 {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(n as usize);
        write(STDOUT, &buf[..len]);
        write(STDOUT, b"\n");
    } else {
        write(STDOUT, b"/\n");
    }
    exit(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { exit(1) }
