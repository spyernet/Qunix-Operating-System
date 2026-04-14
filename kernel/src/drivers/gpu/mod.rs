/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

pub mod console;
use spin::Mutex;

pub struct Framebuffer {
    pub phys_addr: u64,
    pub virt_addr: u64,
    pub width:     u32,
    pub height:    u32,
    pub pitch:     u32,
    pub format:    PixelFormat,
}

#[derive(Clone, Copy)]
pub enum PixelFormat { Rgb32, Bgr32 }

pub static FB: Mutex<Option<Framebuffer>> = Mutex::new(None);

pub fn init(phys_addr: u64, width: u32, height: u32, pitch: u32, fmt: u32) {
    if phys_addr == 0 || width == 0 || height == 0 { return; }
    let format = if fmt == 1 { PixelFormat::Bgr32 } else { PixelFormat::Rgb32 };
    let virt   = map_framebuffer(phys_addr, width, height, pitch);
    *FB.lock() = Some(Framebuffer { phys_addr, virt_addr: virt, width, height, pitch, format });
    crate::klog!("GPU: {}x{} at phys={:#x} virt={:#x}", width, height, phys_addr, virt);
    // Initialize text console on this framebuffer
    console::init();
    // Draw a thin status bar so we know display works
    fill_rect(0, 0, width, 2, 0x004488);
    draw_str(4, 4, "Qunix OS - display active", 0x00FF88, 0x0A0A0A);
}

fn map_framebuffer(phys: u64, w: u32, h: u32, pitch: u32) -> u64 {
    use crate::arch::x86_64::paging::{PageFlags, PageMapper, PAGE_SIZE, KERNEL_VIRT_OFFSET};
    let size   = h as u64 * pitch as u64;
    let pages  = (size + PAGE_SIZE - 1) / PAGE_SIZE;
    let flags  = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    let vbase  = phys + KERNEL_VIRT_OFFSET;
    let mut mapper = PageMapper::current();
    for i in 0..pages {
        let virt = vbase + i * PAGE_SIZE;
        let p    = phys + i * PAGE_SIZE;
        unsafe {
            if mapper.translate(virt).is_none() {
                mapper.map_page(virt, p, flags);
            }
        }
    }
    vbase
}

pub fn phys_addr() -> u64 {
    FB.lock().as_ref().map(|f| f.phys_addr).unwrap_or(0)
}

pub fn fill_rect(x: u32, y: u32, w: u32, h: u32, color: u32) {
    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        let pitch  = fb.pitch / 4;
        let packed = pack_color(color, fb.format);
        for row in y..(y + h).min(fb.height) {
            let base = (fb.virt_addr + row as u64 * fb.pitch as u64) as *mut u32;
            let cols = w.min(fb.width.saturating_sub(x)) as usize;
            unsafe {
                for col in 0..cols { *base.add(x as usize + col) = packed; }
            }
        }
    }
}

pub fn draw_char(x: u32, y: u32, c: u8, fg: u32, bg: u32) {
    let idx = (c as usize).saturating_sub(0x20).min(94);
    let glyph = &FONT_8X16[idx * 16..(idx + 1) * 16];
    for (row, &bits) in glyph.iter().enumerate() {
        for col in 0..8u32 {
            let color = if bits & (0x80 >> col) != 0 { fg } else { bg };
            put_pixel(x + col, y + row as u32, color);
        }
    }
}

pub fn put_pixel(x: u32, y: u32, color: u32) {
    let guard = FB.lock();
    if let Some(fb) = guard.as_ref() {
        if x >= fb.width || y >= fb.height { return; }
        let off    = y * (fb.pitch / 4) + x;
        let packed = pack_color(color, fb.format);
        unsafe { *((fb.virt_addr as *mut u32).add(off as usize)) = packed; }
    }
}

pub fn draw_str(x: u32, y: u32, s: &str, fg: u32, bg: u32) {
    let mut cx = x;
    for c in s.bytes() {
        if cx + 8 > dimensions().0 { break; }
        draw_char(cx, y, c, fg, bg);
        cx += 8;
    }
}

pub fn dimensions() -> (u32, u32) {
    FB.lock().as_ref().map(|f| (f.width, f.height)).unwrap_or((0, 0))
}

pub fn pitch() -> u32 {
    FB.lock().as_ref().map(|f| f.pitch).unwrap_or(0)
}

pub fn format() -> u32 {
    FB.lock().as_ref().map(|f| match f.format { PixelFormat::Bgr32 => 1, _ => 0 }).unwrap_or(0)
}

fn pack_color(rgb: u32, fmt: PixelFormat) -> u32 {
    match fmt {
        PixelFormat::Rgb32 => rgb,
        PixelFormat::Bgr32 => {
            let r = (rgb >> 16) & 0xFF;
            let g = (rgb >>  8) & 0xFF;
            let b =  rgb        & 0xFF;
            (b << 16) | (g << 8) | r
        }
    }
}

static FONT_8X16: &[u8] = include_bytes!("font8x16.bin");
