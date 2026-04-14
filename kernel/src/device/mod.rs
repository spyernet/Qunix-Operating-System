use alloc::vec::Vec;
use spin::Mutex;
use crate::vfs::VfsError;

const ENODEV: VfsError = 19;
const EINVAL: VfsError = 22;

pub type DevReadFn  = fn(*mut u8, usize, u64) -> Result<usize, VfsError>;
pub type DevWriteFn = fn(*const u8, usize, u64) -> Result<usize, VfsError>;

#[derive(Clone)]
pub struct CharDevice {
    pub minor: u32,
    pub name: &'static str,
    pub read: DevReadFn,
    pub write: DevWriteFn,
}

static DEVICES: Mutex<Vec<CharDevice>> = Mutex::new(Vec::new());

pub fn init() {
    register_device(CharDevice {
        minor: 0,
        name: "null",
        read: dev_null_read,
        write: dev_null_write,
    });
    register_device(CharDevice {
        minor: 1,
        name: "zero",
        read: dev_zero_read,
        write: dev_null_write,
    });
    register_device(CharDevice {
        minor: 2,
        name: "tty",
        read: dev_tty_read,
        write: dev_tty_write,
    });
    register_device(CharDevice {
        minor: 3,
        name: "console",
        read: dev_tty_read,
        write: dev_console_write,
    });
    register_device(CharDevice {
        minor: 4,
        name: "serial",
        read: dev_serial_read,
        write: dev_serial_write,
    });
    crate::klog!("Device subsystem initialized");
}

pub fn register_device(dev: CharDevice) {
    DEVICES.lock().push(dev);
}

pub fn read_device(minor: u32, buf: *mut u8, count: usize, offset: u64) -> Result<usize, VfsError> {
    let dev = DEVICES.lock().iter().find(|d| d.minor == minor).cloned();
    match dev {
        Some(d) => (d.read)(buf, count, offset),
        None => Err(ENODEV),
    }
}

pub fn write_device(minor: u32, buf: *const u8, count: usize, offset: u64) -> Result<usize, VfsError> {
    let dev = DEVICES.lock().iter().find(|d| d.minor == minor).cloned();
    match dev {
        Some(d) => (d.write)(buf, count, offset),
        None => Err(ENODEV),
    }
}

fn dev_null_read(_buf: *mut u8, _count: usize, _offset: u64) -> Result<usize, VfsError> {
    Ok(0)
}

fn dev_null_write(_buf: *const u8, count: usize, _offset: u64) -> Result<usize, VfsError> {
    Ok(count)
}

fn dev_zero_read(buf: *mut u8, count: usize, _offset: u64) -> Result<usize, VfsError> {
    unsafe { core::ptr::write_bytes(buf, 0, count); }
    Ok(count)
}

fn dev_tty_read(buf: *mut u8, count: usize, _offset: u64) -> Result<usize, VfsError> {
    // Delegate to the TTY line discipline — blocking, canonical-mode aware.
    crate::tty::tty_read(buf, count, false)
}

fn dev_tty_write(buf: *const u8, count: usize, _offset: u64) -> Result<usize, VfsError> {
    crate::tty::tty_write(buf, count)
}

fn dev_console_write(buf: *const u8, count: usize, _offset: u64) -> Result<usize, VfsError> {
    crate::tty::tty_write(buf, count)
}

fn dev_serial_read(buf: *mut u8, count: usize, _offset: u64) -> Result<usize, VfsError> {
    let mut read = 0;
    loop {
        while read < count {
            match crate::drivers::serial::read_byte() {
                Some(b) => {
                    unsafe { *buf.add(read) = b; }
                    read += 1;
                }
                None => break,
            }
        }
        if read > 0 {
            return Ok(read);
        }
        crate::sched::yield_current();
    }
}

fn dev_serial_write(buf: *const u8, count: usize, _offset: u64) -> Result<usize, VfsError> {
    let slice = unsafe { core::slice::from_raw_parts(buf, count) };
    for &b in slice {
        crate::drivers::serial::write_byte(b);
    }
    Ok(count)
}

// ── Plugin control device (/dev/pluginctl) ────────────────────────────────
//
// Userland uses ioctl() on this device to enable/disable plugins at runtime.
// ioctl numbers:
//   0x5100 = PLUGIN_ENABLE  (arg = ptr to NUL-terminated plugin name)
//   0x5101 = PLUGIN_DISABLE (arg = ptr to NUL-terminated plugin name)
//   0x5102 = PLUGIN_LIST    (arg = ptr to 4096-byte output buffer)

pub const IOCTL_PLUGIN_ENABLE:  u64 = 0x5100;
pub const IOCTL_PLUGIN_DISABLE: u64 = 0x5101;
pub const IOCTL_PLUGIN_LIST:    u64 = 0x5102;

pub fn pluginctl_ioctl(request: u64, arg: u64) -> i64 {
    match request {
        IOCTL_PLUGIN_ENABLE => {
            if arg == 0 { return -22; }
            let name = unsafe {
                let mut len = 0;
                while *((arg + len) as *const u8) != 0 && len < 256 { len += 1; }
                core::str::from_utf8_unchecked(core::slice::from_raw_parts(arg as *const u8, len as usize))
            };
            if crate::plugins::runtime_enable(name) { 0 } else { -2 }
        }
        IOCTL_PLUGIN_DISABLE => {
            if arg == 0 { return -22; }
            let name = unsafe {
                let mut len = 0;
                while *((arg + len) as *const u8) != 0 && len < 256 { len += 1; }
                core::str::from_utf8_unchecked(core::slice::from_raw_parts(arg as *const u8, len as usize))
            };
            if crate::plugins::runtime_disable(name) { 0 } else { -2 }
        }
        IOCTL_PLUGIN_LIST => {
            if arg == 0 { return -22; }
            let content = crate::plugins::proc_plugins_content();
            let len = content.len().min(4096);
            unsafe { core::ptr::copy_nonoverlapping(content.as_ptr(), arg as *mut u8, len); }
            len as i64
        }
        _ => -22,
    }
}
