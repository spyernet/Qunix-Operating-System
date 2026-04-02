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
}

static CON: Mutex<Console> = Mutex::new(Console {
    col: 0, row: 0, cols: 0, rows: 0,
    fg: 0xE0E0E0, bg: 0x0A0A0A,
    ready: false,
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

pub fn write_char(byte: u8) {
    let mut c = CON.lock();
    if !c.ready { return; }
    match byte {
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
        0x08 /* BS */ => {
            if c.col > 0 { c.col -= 1; }
            let (x, y, fg, bg) = (c.col * CHAR_W, c.row * CHAR_H, c.fg, c.bg);
            drop(c);
            super::draw_char(x, y, b' ', fg, bg);
            return;
        }
        0x1B /* ESC */ => { return; } // skip ANSI escape starts (no full ANSI here)
        _ => {
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
    }
}

pub fn write_str(s: &str) {
    for b in s.bytes() { write_char(b); }
}

fn scroll_up(c: &mut Console) {
    let (w, h) = super::dimensions();
    let line_h = CHAR_H;
    // Blit all rows up by one
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
