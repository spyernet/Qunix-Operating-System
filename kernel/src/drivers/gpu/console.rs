//! Framebuffer text console.
//! Provides a scrolling terminal on the UEFI linear framebuffer.
//! Used by the kernel logger and early userspace before a real terminal driver.

use spin::Mutex;

const CHAR_W: u32 = 8;
const CHAR_H: u32 = 16;

struct Console {
    col:    u32,
    row:    u32,
    cols:   u32,
    rows:   u32,
    fg:     u32,
    bg:     u32,
    ready:  bool,
    // ANSI escape sequence state machine
    esc_state: u8,   // 0=normal, 1=saw ESC, 2=in CSI (ESC [)
    esc_buf:   [u8; 32],
    esc_len:   usize,
}

static CON: Mutex<Console> = Mutex::new(Console {
    col: 0, row: 0, cols: 0, rows: 0,
    fg: 0xE0E0E0, bg: 0x0A0A0A,
    ready: false,
    esc_state: 0,
    esc_buf:   [0u8; 32],
    esc_len:   0,
});

pub fn init() {
    let (w, h) = super::dimensions();
    if w == 0 { return; }
    let mut c = CON.lock();
    c.cols  = w / CHAR_W;
    c.rows  = h / CHAR_H;
    c.ready = true;
    // Fill background
    super::fill_rect(0, 0, w, h, 0x0A0A0A);
}

pub fn is_ready() -> bool { CON.lock().ready }

/// Parse a run of ASCII digits from `buf` starting at `i`.
/// Returns (parsed_number, new_i).
fn parse_num(buf: &[u8], mut i: usize) -> (u32, usize) {
    let mut n = 0u32;
    while i < buf.len() && buf[i] >= b'0' && buf[i] <= b'9' {
        n = n * 10 + (buf[i] - b'0') as u32;
        i += 1;
    }
    (n, i)
}

pub fn write_char(byte: u8) {
    let mut c = CON.lock();
    if !c.ready { return; }

    // ── ANSI escape sequence state machine ───────────────────────────────
    if c.esc_state == 1 {
        if byte == b'[' {
            c.esc_state = 2;
            c.esc_len = 0;
        } else {
            c.esc_state = 0; // unrecognised — discard
        }
        return;
    }

    if c.esc_state == 2 {
        if byte.is_ascii_alphabetic() {
            // Terminating byte — process if it's 'm' (SGR), discard otherwise.
            if byte == b'm' {
                // Copy the param bytes out so we release the borrow on c.esc_buf
                // before we start mutating c.fg / c.bg.
                let len = c.esc_len;
                let mut params = [0u8; 32];
                params[..len].copy_from_slice(&c.esc_buf[..len]);

                let mut i = 0usize;
                // An empty sequence "ESC [ m" means reset (code 0).
                if len == 0 {
                    c.fg = 0xCCCCCC;
                    c.bg = 0x000000;
                }
                while i < len {
                    let (n, ni) = parse_num(&params, i);
                    i = ni;
                    if i < len && params[i] == b';' { i += 1; }
                    match n {
                        0  => { c.fg = 0xCCCCCC; c.bg = 0x000000; }
                        1  => { c.fg |= 0x808080; }   // bold → brighten fg
                        30 => c.fg = 0x000000,
                        31 => c.fg = 0xCC0000,
                        32 => c.fg = 0x00CC00,
                        33 => c.fg = 0xCCCC00,
                        34 => c.fg = 0x0000CC,
                        35 => c.fg = 0xCC00CC,
                        36 => c.fg = 0x00CCCC,
                        37 => c.fg = 0xCCCCCC,
                        90 => c.fg = 0x555555,
                        91 => c.fg = 0xFF5555,
                        92 => c.fg = 0x55FF55,
                        93 => c.fg = 0xFFFF55,
                        94 => c.fg = 0x5555FF,
                        95 => c.fg = 0xFF55FF,
                        96 => c.fg = 0x55FFFF,
                        97 => c.fg = 0xFFFFFF,
                        _  => {}
                    }
                }
            }
            c.esc_state = 0;
            c.esc_len   = 0;
        } else {
            // Accumulate parameter byte — local var avoids simultaneous
            // mutable + immutable borrow of c.esc_buf / c.esc_len.
            let len = c.esc_len;
            if len < c.esc_buf.len() {
                c.esc_buf[len] = byte;
                c.esc_len = len + 1;
            }
        }
        return;
    }

    // ── Normal character processing ───────────────────────────────────────
    match byte {
        0x1B /* ESC */ => { c.esc_state = 1; }
        b'\r' => { c.col = 0; }
        b'\n' => {
            c.col = 0;
            c.row += 1;
            if c.row >= c.rows { scroll_up(&mut c); }
        }
        b'\t' => {
            let next = (c.col + 8) & !7;
            c.col = next.min(c.cols - 1);
        }
        0x08 /* BS */ | 0x7F /* DEL */ => {
            if c.col > 0 { c.col -= 1; }
            let (x, y, fg, bg) = (c.col * CHAR_W, c.row * CHAR_H, c.fg, c.bg);
            drop(c);
            super::draw_char(x, y, b' ', fg, bg);
            return;
        }
        _ if byte >= 0x20 => {
            let (x, y, fg, bg) = (c.col * CHAR_W, c.row * CHAR_H, c.fg, c.bg);
            drop(c);
            super::draw_char(x, y, byte, fg, bg);
            let mut c = CON.lock();
            c.col += 1;
            if c.col >= c.cols {
                c.col = 0;
                c.row += 1;
                if c.row >= c.rows { scroll_up(&mut c); }
            }
            return;
        }
        _ => {} // ignore other control chars
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() { write_char(b); }
}

fn scroll_up(c: &mut Console) {
    let (w, h) = super::dimensions();
    let _ = h;
    let line_h = CHAR_H;
    let guard = super::FB.lock();
    if let Some(fb) = guard.as_ref() {
        let pitch = fb.pitch as usize;
        let copy_bytes = w as usize * 4;
        let rows_to_move = (c.rows - 1) as usize;
        unsafe {
            let base = fb.virt_addr as *mut u8;
            for row in 0..rows_to_move {
                let src = base.add((row + 1) * line_h as usize * pitch);
                let dst = base.add(row * line_h as usize * pitch);
                for line in 0..line_h as usize {
                    core::ptr::copy_nonoverlapping(
                        src.add(line * pitch),
                        dst.add(line * pitch),
                        copy_bytes,
                    );
                }
            }
            // Clear last row
            let last_row = base.add(rows_to_move * line_h as usize * pitch);
            let clear_size = line_h as usize * pitch;
            core::ptr::write_bytes(last_row, 0x0A, clear_size);
        }
    }
    drop(guard);
    c.row = c.rows - 1;
}
