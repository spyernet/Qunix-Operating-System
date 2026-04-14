//! TTY / line-discipline subsystem.
//!
//! Provides one software TTY (backed by the keyboard + VGA/serial).
//! Shells interact with it through /dev/tty (minor 2) / /dev/console (minor 3).
//!
//! ## Line discipline (canonical mode)
//!
//! In canonical mode (the default), input is buffered line-by-line.
//! Characters are echoed as they are typed; backspace erases the previous
//! character both from the buffer and from the screen.  A complete line
//! (terminated by `\n` or `\r`) is delivered to readers.
//!
//! Raw mode (ICANON cleared via TCSETS) passes bytes straight through.
//!
//! ## Termios
//!
//! We store a `Termios` struct per TTY.  The full POSIX struct is 60 bytes;
//! we only interpret the flag bits that matter for shell interaction.
//!
//! ## Wait queue
//!
//! Processes blocked in `tty_read` register in `TTY_READERS`.
//! The keyboard IRQ calls `tty_wake_readers()` after depositing input.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use spin::Mutex;
use crate::process::Pid;
use crate::vfs::VfsError;

// ── Termios constants ─────────────────────────────────────────────────────
// (Linux x86-64 values)
pub const ICANON:  u32 = 0x0002;  // canonical mode
pub const ECHO:    u32 = 0x0008;  // echo input
pub const ECHOE:   u32 = 0x0010;  // echo ERASE as BS-SP-BS
pub const ISIG:    u32 = 0x0001;  // generate signals
pub const OPOST:   u32 = 0x0001;  // output processing
pub const ONLCR:   u32 = 0x0004;  // map NL → CR-NL on output

// c_cc indices
pub const VINTR:   usize = 0;
pub const VQUIT:   usize = 1;
pub const VERASE:  usize = 2;
pub const VKILL:   usize = 3;
pub const VEOF:    usize = 4;
pub const VTIME:   usize = 5;
pub const VMIN:    usize = 6;
pub const VSUSP:   usize = 10;

/// Linux `struct termios` layout (x86-64).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Termios {
    pub c_iflag: u32,
    pub c_oflag: u32,
    pub c_cflag: u32,
    pub c_lflag: u32,
    pub c_line:  u8,
    pub c_cc:    [u8; 19],
}

impl Termios {
    pub const fn default_cooked() -> Self {
        let mut t = Termios {
            c_iflag: 0x0500,  // ICRNL | IXON
            c_oflag: OPOST | ONLCR,
            c_cflag: 0x00BF,  // B38400 | CS8 | CREAD | HUPCL
            c_lflag: ICANON | ECHO | ECHOE | ISIG | 0x8000, // +IEXTEN
            c_line:  0,
            c_cc:    [0u8; 19],
        };
        t.c_cc[VINTR]  = 0x03; // ^C
        t.c_cc[VQUIT]  = 0x1C; // ^\ 
        t.c_cc[VERASE] = 0x7F; // DEL (or BS)
        t.c_cc[VKILL]  = 0x15; // ^U
        t.c_cc[VEOF]   = 0x04; // ^D
        t.c_cc[VTIME]  = 0;
        t.c_cc[VMIN]   = 1;
        t.c_cc[VSUSP]  = 0x1A; // ^Z
        t
    }
}

/// `struct winsize` (TIOCGWINSZ / TIOCSWINSZ)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Winsize {
    pub ws_row:    u16,
    pub ws_col:    u16,
    pub ws_xpixel: u16,
    pub ws_ypixel: u16,
}

// ── TTY state ─────────────────────────────────────────────────────────────

struct TtyState {
    /// Current termios settings.
    termios: Termios,
    /// Window size.
    winsize: Winsize,
    /// Foreground process group (job control).
    foreground_pgid: u32,
    /// Controlling session.
    session: u32,
    /// Line buffer (canonical mode only).
    /// Characters accumulate here until Enter; then a full line is
    /// moved to `ready_lines`.
    line_buf: Vec<u8>,
    /// Complete lines ready to be read.
    ready_lines: VecDeque<Vec<u8>>,
    /// Raw-mode byte queue (non-canonical).
    raw_buf: VecDeque<u8>,
}

impl TtyState {
    const fn new() -> Self {
        TtyState {
            termios:         Termios::default_cooked(),
            winsize:         Winsize { ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 },
            foreground_pgid: 1,
            session:         1,
            line_buf:        Vec::new(),
            ready_lines:     VecDeque::new(),
            raw_buf:         VecDeque::new(),
        }
    }

    fn is_canonical(&self) -> bool { self.termios.c_lflag & ICANON != 0 }
    fn echo_enabled(&self) -> bool  { self.termios.c_lflag & ECHO   != 0 }
    fn echoe_enabled(&self) -> bool { self.termios.c_lflag & ECHOE  != 0 }
    fn isig_enabled(&self) -> bool  { self.termios.c_lflag & ISIG   != 0 }

    fn erase_char(&self) -> u8 { self.termios.c_cc[VERASE] }
    fn kill_char(&self)  -> u8 { self.termios.c_cc[VKILL]  }
    fn eof_char(&self)   -> u8 { self.termios.c_cc[VEOF]   }
    fn intr_char(&self)  -> u8 { self.termios.c_cc[VINTR]  }

    /// Feed one byte from the keyboard into the line discipline.
    /// Returns the signal to deliver (if any): 2=SIGINT, 20=SIGTSTP, etc.
    fn input_byte(&mut self, byte: u8) -> Option<u32> {
        // Signal characters (only in canonical+isig mode)
        if self.is_canonical() && self.isig_enabled() {
            if byte == self.intr_char() {
                // Echo ^C
                if self.echo_enabled() {
                    tty_echo(b'^'); tty_echo(b'C'); tty_echo(b'\n');
                }
                self.line_buf.clear();
                return Some(crate::signal::SIGINT);
            }
            let susp = self.termios.c_cc[VSUSP];
            if susp != 0 && byte == susp {
                if self.echo_enabled() {
                    tty_echo(b'^'); tty_echo(b'Z'); tty_echo(b'\n');
                }
                self.line_buf.clear();
                return Some(crate::signal::SIGTSTP);
            }
        }

        if self.is_canonical() {
            // ERASE (backspace)
            let erase = self.erase_char();
            if byte == erase || byte == b'\x08' {
                if !self.line_buf.is_empty() {
                    self.line_buf.pop();
                    if self.echoe_enabled() {
                        // Erase: BS space BS
                        tty_echo(b'\x08'); tty_echo(b' '); tty_echo(b'\x08');
                    }
                }
                return None;
            }

            // KILL (^U — erase entire line)
            let kill = self.kill_char();
            if kill != 0 && byte == kill {
                while !self.line_buf.is_empty() {
                    self.line_buf.pop();
                    if self.echoe_enabled() {
                        tty_echo(b'\x08'); tty_echo(b' '); tty_echo(b'\x08');
                    }
                }
                return None;
            }

            // EOF (^D) — flush current line_buf as a line (possibly empty → EOF)
            let eof = self.eof_char();
            if eof != 0 && byte == eof {
                let line = core::mem::take(&mut self.line_buf);
                // Empty line on ^D signals EOF to the reader
                self.ready_lines.push_back(line);
                return None;
            }

            // Normal character
            if self.echo_enabled() {
                // Translate \r to \n for echo if ICRNL
                let echo_byte = if byte == b'\r' { b'\n' } else { byte };
                tty_echo(echo_byte);
            }

            let ch = if byte == b'\r' { b'\n' } else { byte };
            self.line_buf.push(ch);

            // Newline completes the line
            if ch == b'\n' {
                let line = core::mem::take(&mut self.line_buf);
                self.ready_lines.push_back(line);
            }
        } else {
            // Raw mode — just buffer the byte
            if self.echo_enabled() { tty_echo(byte); }
            self.raw_buf.push_back(byte);
        }

        None
    }

    /// Read up to `count` bytes.  Returns `None` if no data is available.
    fn try_read(&mut self, buf: &mut [u8]) -> Option<usize> {
        if self.is_canonical() {
            let line = self.ready_lines.front_mut()?;
            let n = line.len().min(buf.len());
            buf[..n].copy_from_slice(&line[..n]);
            if n == line.len() {
                self.ready_lines.pop_front();
            } else {
                let _ = line.drain(..n);
            }
            Some(n)
        } else {
            if self.raw_buf.is_empty() { return None; }
            let n = self.raw_buf.len().min(buf.len());
            for i in 0..n { buf[i] = self.raw_buf.pop_front().unwrap(); }
            Some(n)
        }
    }

    fn data_available(&self) -> bool {
        if self.is_canonical() {
            !self.ready_lines.is_empty()
        } else {
            !self.raw_buf.is_empty()
        }
    }
}

// ── Global TTY ─────────────────────────────────────────────────────────────

static TTY: Mutex<TtyState> = Mutex::new(TtyState::new());

// Wait queue: PIDs blocked waiting for TTY input
static TTY_READERS: Mutex<Vec<Pid>> = Mutex::new(Vec::new());

fn tty_echo(byte: u8) {
    crate::drivers::vga::write_byte(byte);
    crate::drivers::serial::write_byte(byte);
}

fn handle_input_byte(byte: u8) {
    let signal_opt = TTY.lock().input_byte(byte);

    // Deliver signal to foreground process group
    if let Some(sig) = signal_opt {
        let pgid = TTY.lock().foreground_pgid;
        crate::signal::send_signal_group(pgid, sig);
    }
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Called by the keyboard IRQ handler when a byte arrives.
/// Feeds it through the line discipline, optionally wakes readers.
pub fn tty_input_byte(byte: u8) {
    handle_input_byte(byte);

    // Wake any readers
    tty_wake_readers();
}

/// Drain bytes from the serial RX FIFO into the TTY line discipline.
///
/// `run_qemu.sh` uses `-serial stdio`, so headless shells receive input over
/// COM1 instead of the PS/2 keyboard path. Poll serial on the timer tick so
/// `/dev/tty` behaves like the advertised keyboard+serial console.
pub fn poll_input_devices() {
    let mut saw_input = false;
    while let Some(byte) = crate::drivers::serial::read_byte() {
        saw_input = true;
        handle_input_byte(byte);
    }
    if saw_input {
        tty_wake_readers();
    }
}

/// Wake all processes blocked in tty_read.
pub fn tty_wake_readers() {
    let pids: Vec<Pid> = core::mem::take(&mut *TTY_READERS.lock());
    for pid in pids {
        crate::sched::wake_process(pid);
    }
}

/// Register a PID as a poll() waiter for TTY readability.
/// Used by sys_poll when a TTY fd is being polled — pid will be woken
/// by tty_wake_readers() when input becomes available.
pub fn register_poll_waiter(pid: Pid) {
    let mut readers = TTY_READERS.lock();
    // Avoid duplicate registration
    if !readers.contains(&pid) {
        readers.push(pid);
    }
}

/// Blocking TTY read — used by dev_tty_read in device/mod.rs.
pub fn tty_read(buf: *mut u8, count: usize, nonblock: bool) -> Result<usize, VfsError> {
    let slice = unsafe { core::slice::from_raw_parts_mut(buf, count) };
    loop {
        let pid = crate::process::current_pid();

        // Register as a waiter BEFORE checking for data.
        //
        // Lost-wakeup prevention: if we checked for data first and then
        // registered, a keyboard IRQ could arrive in the window between
        // the check (found nothing) and the registration.  The IRQ would
        // call tty_wake_readers() on an empty list, deposit its byte, and
        // then nobody would ever wake us again.
        //
        // By registering first, the IRQ is guaranteed to see our PID in the
        // list, call wake_process() for us, and we will retry the read after
        // waking — at which point the byte is in the buffer.
        {
            let mut readers = TTY_READERS.lock();
            if !readers.contains(&pid) {
                readers.push(pid);
            }
        }

        // Now try to read.  If data is already available (typed before we
        // blocked, or deposited by an IRQ between our registration and this
        // check), consume it and remove our registration.
        if let Some(n) = TTY.lock().try_read(slice) {
            TTY_READERS.lock().retain(|&p| p != pid);
            return Ok(n);
        }

        if nonblock {
            TTY_READERS.lock().retain(|&p| p != pid);
            return Err(crate::vfs::EAGAIN);
        }

        // Block.  The keyboard IRQ will call tty_wake_readers() →
        // wake_process(pid) → set NEED_RESCHED → schedule() picks us back
        // up on the next timer tick or immediately if we are the only task.
        crate::sched::block_current(crate::process::ProcessState::Sleeping);
        // Woken — retry from the top to re-register and re-check.
    }
}

/// Write to TTY output (with ONLCR translation if set).
pub fn tty_write(buf: *const u8, count: usize) -> Result<usize, VfsError> {
    let slice = unsafe { core::slice::from_raw_parts(buf, count) };
    let onlcr = TTY.lock().termios.c_oflag & ONLCR != 0;
    for &b in slice {
        if onlcr && b == b'\n' {
            crate::drivers::vga::write_byte(b'\r');
            crate::drivers::serial::write_byte(b'\r');
            if crate::drivers::gpu::console::is_ready() {
                crate::drivers::gpu::console::write_char(b'\r');
            }
        }
        crate::drivers::vga::write_byte(b);
        crate::drivers::serial::write_byte(b);
        // Also route output to the GPU framebuffer console so the shell
        // prompt and command output are visible on the 1280x800 display.
        if crate::drivers::gpu::console::is_ready() {
            crate::drivers::gpu::console::write_char(b);
        }
    }
    Ok(count)
}

/// Returns true if `count` bytes are available without blocking.
pub fn tty_poll_readable() -> bool { TTY.lock().data_available() }

// ── ioctl handlers ────────────────────────────────────────────────────────

/// Linux ioctl numbers for TTY (x86-64).
pub const TCGETS:    u64 = 0x5401;
pub const TCSETS:    u64 = 0x5402;
pub const TCSETSW:   u64 = 0x5403;
pub const TCSETSF:   u64 = 0x5404;
pub const TIOCGPGRP: u64 = 0x540F;
pub const TIOCSPGRP: u64 = 0x5410;
pub const TIOCGWINSZ:u64 = 0x5413;
pub const TIOCSWINSZ:u64 = 0x5414;
pub const TIOCSCTTY: u64 = 0x540E;
pub const TIOCGCTTY: u64 = 0x5439;
pub const TIOCNOTTY: u64 = 0x5422;
pub const TIOCEXCL:  u64 = 0x540C;
pub const TIOCNXCL:  u64 = 0x540D;

pub fn tty_ioctl(fd: i32, req: u64, arg: u64) -> i64 {
    match req {
        TCGETS => {
            if arg == 0 { return -14; } // EFAULT
            let t = TTY.lock().termios;
            unsafe { *(arg as *mut Termios) = t; }
            0
        }
        TCSETS | TCSETSW | TCSETSF => {
            if arg == 0 { return -14; }
            let new_t = unsafe { *(arg as *const Termios) };
            TTY.lock().termios = new_t;
            0
        }
        TIOCGPGRP => {
            if arg == 0 { return -14; }
            let pgid = TTY.lock().foreground_pgid;
            unsafe { *(arg as *mut u32) = pgid; }
            0
        }
        TIOCSPGRP => {
            if arg == 0 { return -14; }
            let pgid = unsafe { *(arg as *const u32) };
            TTY.lock().foreground_pgid = pgid;
            0
        }
        TIOCGWINSZ => {
            if arg == 0 { return -14; }
            let ws = TTY.lock().winsize;
            unsafe { *(arg as *mut Winsize) = ws; }
            0
        }
        TIOCSWINSZ => {
            if arg == 0 { return -14; }
            let ws = unsafe { *(arg as *const Winsize) };
            TTY.lock().winsize = ws;
            // Send SIGWINCH to foreground process group
            let pgid = TTY.lock().foreground_pgid;
            crate::signal::send_signal_group(pgid, crate::signal::SIGWINCH);
            0
        }
        TIOCSCTTY => {
            // Make this TTY the controlling terminal of the current session
            let sid = crate::process::with_current(|p| p.sid).unwrap_or(0);
            TTY.lock().session = sid;
            crate::process::with_current_mut(|p| { p.tty = 1; });
            0
        }
        TIOCNOTTY => {
            crate::process::with_current_mut(|p| { p.tty = -1; });
            0
        }
        TIOCEXCL | TIOCNXCL => 0,  // exclusive mode — no-op
        TIOCGCTTY => 0,
        _ => -22, // EINVAL — unknown ioctl
    }
}

/// Returns true if the given fd is a TTY (minor 2, 3, or 4).
pub fn is_tty_fd(fd: i32) -> bool {
    crate::process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| matches!(
            &f.kind,
            crate::vfs::FdKind::Device(2) |
            crate::vfs::FdKind::Device(3) |
            crate::vfs::FdKind::Device(4)
        )).unwrap_or(false)
    }).unwrap_or(false)
}

/// Set the foreground process group (used by setsid / shell init).
pub fn set_foreground_pgid(pgid: u32) {
    TTY.lock().foreground_pgid = pgid;
}

pub fn get_foreground_pgid() -> u32 {
    TTY.lock().foreground_pgid
}
