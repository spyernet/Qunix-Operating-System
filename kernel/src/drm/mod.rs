/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Qunix DRM/KMS subsystem — real modesetting backed by UEFI framebuffer.
//!
//! Architecture:
//!   Display pipeline: CRTC → Encoder → Connector → Panel
//!   Framebuffer: GEM dumb buffers backed by physical frames
//!   Scanout: software blit of active framebuffer to UEFI linear framebuffer
//!   Planes: primary + cursor plane per CRTC
//!
//! X11/Wayland/DRM userland interface:
//!   - DRM_IOCTL_MODE_GETRESOURCES: 1 CRTC, 1 connector, 1 encoder, 1 plane
//!   - CREATE_DUMB/MAP_DUMB/ADDFB2/PAGE_FLIP: full path implemented
//!   - MODE_ATOMIC: property-based commit dispatched to real scanout
//!   - Cursor: rendered into a separate layer and composited on blit

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use crate::memory::phys::{alloc_frames, free_frames_n};
use crate::arch::x86_64::paging::{phys_to_virt, PAGE_SIZE};

// ── Display mode ──────────────────────────────────────────────────────────

/// Exact layout of struct drm_mode_modeinfo (libdrm/drm_mode.h)
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct ModeInfo {
    pub clock:       u32,
    pub hdisplay:    u16, pub hsync_start: u16, pub hsync_end: u16, pub htotal: u16, pub hskew: u16,
    pub vdisplay:    u16, pub vsync_start: u16, pub vsync_end: u16, pub vtotal: u16, pub vscan: u16,
    pub vrefresh:    u32,
    pub flags:       u32,
    pub mode_type:   u32,
    pub name:        [u8; 32],
}

impl ModeInfo {
    pub fn from_resolution(w: u32, h: u32, refresh: u32) -> Self {
        let mut m = ModeInfo::default();
        // CVT-approximate timings
        m.clock       = (w * h * refresh + 999) / 1000;
        m.hdisplay    = w as u16;
        m.hsync_start = (w + (w / 8)) as u16;
        m.hsync_end   = (w + (w / 8) + 8) as u16;
        m.htotal      = (w + (w / 4)) as u16;
        m.vdisplay    = h as u16;
        m.vsync_start = (h + 3) as u16;
        m.vsync_end   = (h + 8) as u16;
        m.vtotal      = (h + 28) as u16;
        m.vrefresh    = refresh;
        m.flags       = 0xA; // NHSYNC | NVSYNC
        m.mode_type   = 0x48; // PREFERRED | DRIVER
        let name = alloc::format!("{}x{}", w, h);
        let nb = name.as_bytes();
        m.name[..nb.len().min(31)].copy_from_slice(&nb[..nb.len().min(31)]);
        m
    }
}

// ── GEM buffer object ─────────────────────────────────────────────────────

pub struct GemBo {
    pub handle: u32,
    pub phys:   u64,
    pub size:   u64,
    pub width:  u32,
    pub height: u32,
    pub pitch:  u32,
    pub bpp:    u32,
}

impl GemBo {
    pub fn virt(&self) -> u64 { phys_to_virt(self.phys) }

    pub fn clear(&self) {
        unsafe { core::ptr::write_bytes(self.virt() as *mut u8, 0, self.size as usize); }
    }
}

// ── Framebuffer ───────────────────────────────────────────────────────────

pub struct DrmFb {
    pub id:     u32,
    pub width:  u32,
    pub height: u32,
    pub pitch:  u32,
    pub bpp:    u32,
    pub format: u32,  // DRM fourcc
    pub handle: u32,  // GEM handle
}

// ── Plane state ───────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct PlaneState {
    pub fb_id:   u32,
    pub crtc_id: u32,
    pub src_x:   u32, pub src_y:   u32, pub src_w:   u32, pub src_h:   u32,
    pub crtc_x:  i32, pub crtc_y:  i32, pub crtc_w:  u32, pub crtc_h:  u32,
    pub visible: bool,
}

// ── CRTC state ────────────────────────────────────────────────────────────

pub struct Crtc {
    pub id:         u32,
    pub fb_id:      u32,
    pub x:          u32,
    pub y:          u32,
    pub mode:       ModeInfo,
    pub mode_valid: bool,
    pub active:     bool,
    pub mode_blob:  u32,
    // Cursor state
    pub cursor_fb:  Option<u32>,
    pub cursor_x:   i32,
    pub cursor_y:   i32,
}

// ── Property registry ─────────────────────────────────────────────────────

// Property IDs — must stay stable for X11/Wayland
const PROP_ACTIVE:     u32 = 1;
const PROP_MODE_ID:    u32 = 2;
const PROP_FB_ID:      u32 = 3;
const PROP_CRTC_ID:    u32 = 4;
const PROP_SRC_X:      u32 = 5;
const PROP_SRC_Y:      u32 = 6;
const PROP_SRC_W:      u32 = 7;
const PROP_SRC_H:      u32 = 8;
const PROP_CRTC_X:     u32 = 9;
const PROP_CRTC_Y:     u32 = 10;
const PROP_CRTC_W:     u32 = 11;
const PROP_CRTC_H:     u32 = 12;
const PROP_TYPE:       u32 = 13;
const PROP_IN_FORMATS: u32 = 14;

// Object types
const DRM_MODE_OBJECT_CRTC:      u32 = 0xCCCCCCCC;
const DRM_MODE_OBJECT_CONNECTOR: u32 = 0xC0C0C0C0;
const DRM_MODE_OBJECT_ENCODER:   u32 = 0xE0E0E0E0;
const DRM_MODE_OBJECT_PLANE:     u32 = 0xEEEEEEEE;
const DRM_MODE_OBJECT_FB:        u32 = 0xFBFBFBFB;
const DRM_MODE_OBJECT_BLOB:      u32 = 0xBBBBBBBB;

// ── Main device state ─────────────────────────────────────────────────────

struct DrmDevice {
    // Physical framebuffer from UEFI
    fb_phys:    u64,
    fb_virt:    u64,
    fb_width:   u32,
    fb_height:  u32,
    fb_pitch:   u32,
    fb_format:  u32,   // 0=RGB, 1=BGR

    // KMS state
    crtc:       Crtc,
    primary:    PlaneState,
    cursor:     PlaneState,

    // GEM objects
    gems:       BTreeMap<u32, GemBo>,
    next_handle: u32,

    // Framebuffers
    fbs:        BTreeMap<u32, DrmFb>,
    next_fb_id: u32,

    // Property blobs (mode blobs etc.)
    blobs:      BTreeMap<u32, Vec<u8>>,
    next_blob:  u32,

    // Sync objects
    next_syncobj: u32,

    master:     bool,

    // Pending vblank events (fd-indexed)
    pending_flip: bool,
}

impl DrmDevice {
    fn new(fb_phys: u64, fb_virt: u64, w: u32, h: u32, pitch: u32, fmt: u32) -> Self {
        let mode = ModeInfo::from_resolution(w.max(640), h.max(480), 60);
        DrmDevice {
            fb_phys, fb_virt, fb_width: w, fb_height: h, fb_pitch: pitch, fb_format: fmt,
            crtc: Crtc {
                id: 1, fb_id: 0, x: 0, y: 0, mode,
                mode_valid: w > 0, active: w > 0, mode_blob: 0,
                cursor_fb: None, cursor_x: 0, cursor_y: 0,
            },
            primary: PlaneState { crtc_id: 1, visible: true, ..Default::default() },
            cursor:  PlaneState::default(),
            gems:       BTreeMap::new(),
            next_handle: 1,
            fbs:        BTreeMap::new(),
            next_fb_id: 1,
            blobs:      BTreeMap::new(),
            next_blob:  1,
            next_syncobj: 1,
            master: false,
            pending_flip: false,
        }
    }

    fn alloc_handle(&mut self) -> u32 { let h = self.next_handle; self.next_handle += 1; h }
    fn alloc_fb_id(&mut self) -> u32  { let id = self.next_fb_id; self.next_fb_id += 1; id }
    fn alloc_blob(&mut self) -> u32   { let id = self.next_blob; self.next_blob += 1; id }

    fn create_dumb(&mut self, w: u32, h: u32, bpp: u32) -> Option<u32> {
        let bytes_pp = (bpp + 7) / 8;
        let pitch    = ((w * bytes_pp + 63) & !63).max(w * bytes_pp);
        let size     = ((pitch as u64 * h as u64) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let pages    = size / PAGE_SIZE;
        let phys     = alloc_frames(pages as usize)?;
        unsafe { core::ptr::write_bytes(phys_to_virt(phys) as *mut u8, 0, size as usize); }
        let handle = self.alloc_handle();
        self.gems.insert(handle, GemBo { handle, phys, size, width: w, height: h, pitch, bpp });
        Some(handle)
    }

    fn free_gem(&mut self, handle: u32) {
        if let Some(bo) = self.gems.remove(&handle) {
            if bo.phys != 0 { free_frames_n(bo.phys, (bo.size / PAGE_SIZE) as usize); }
        }
    }

    /// Scanout: blit the active framebuffer GEM to the UEFI linear framebuffer.
    /// This is the "page flip" / "mode set" — the actual display update.
    fn scanout(&self) {
        let fb_id = self.crtc.fb_id;
        if fb_id == 0 { return; }
        let fb = match self.fbs.get(&fb_id) { Some(f) => f, None => return };
        let bo = match self.gems.get(&fb.handle) { Some(b) => b, None => return };

        let (disp_w, disp_h) = (self.fb_width, self.fb_height);
        if disp_w == 0 || disp_h == 0 || self.fb_virt == 0 { return; }

        let blit_w   = fb.width.min(disp_w) as usize;
        let blit_h   = fb.height.min(disp_h) as usize;
        let src_pp   = (fb.bpp + 7) / 8;
        let src_pitch = fb.pitch as usize;
        let dst_pitch = self.fb_pitch as usize;
        let dst_bpp   = 4usize; // UEFI always 32bpp

        // Source format: check fourcc
        let need_swap = self.fb_format == 1; // BGR display
        let src_bgr   = matches!(fb.format, 0x34324258 | 0x34324241); // XBGR/ABGR

        for row in 0..blit_h {
            let src_row = bo.virt() + (row * src_pitch) as u64;
            let dst_row = self.fb_virt + (row * dst_pitch) as u64;
            unsafe {
                if src_pp == 4 && dst_bpp == 4 && !need_swap && !src_bgr {
                    // Fast path: direct 32bpp copy
                    core::ptr::copy_nonoverlapping(
                        src_row as *const u8, dst_row as *mut u8, blit_w * 4
                    );
                } else {
                    // Slow path: per-pixel format conversion
                    for col in 0..blit_w {
                        let src_px = src_row + (col * src_pp as usize) as u64;
                        let dst_px = dst_row + (col * dst_bpp) as u64;
                        let mut px = *(src_px as *const u32);
                        // Source is XBGR → convert to XRGB
                        if src_bgr { px = swap_rb(px); }
                        // Display is BGR → convert XRGB to XBGR
                        if need_swap { px = swap_rb(px); }
                        *(dst_px as *mut u32) = px;
                    }
                }
            }
        }

        // Composite cursor if visible
        if let Some(cursor_fb_id) = self.crtc.cursor_fb {
            self.blit_cursor(cursor_fb_id);
        }
    }

    fn blit_cursor(&self, cursor_fb_id: u32) {
        let fb = match self.fbs.get(&cursor_fb_id) { Some(f) => f, None => return };
        let bo = match self.gems.get(&fb.handle) { Some(b) => b, None => return };
        let cx = self.crtc.cursor_x.max(0) as u32;
        let cy = self.crtc.cursor_y.max(0) as u32;
        let cw = fb.width.min(self.fb_width.saturating_sub(cx));
        let ch = fb.height.min(self.fb_height.saturating_sub(cy));
        for row in 0..ch as usize {
            for col in 0..cw as usize {
                let src_px = bo.virt() + (row * fb.pitch as usize + col * 4) as u64;
                unsafe {
                    let px = *(src_px as *const u32);
                    let a  = (px >> 24) & 0xFF;
                    if a == 0 { continue; }
                    let dst_row = (cy as usize + row) * self.fb_pitch as usize;
                    let dst_col = (cx as usize + col) * 4;
                    let dst     = (self.fb_virt + (dst_row + dst_col) as u64) as *mut u32;
                    // Alpha blend
                    if a == 0xFF {
                        let mut out = px & 0x00FFFFFF;
                        if self.fb_format == 1 { out = swap_rb(out); }
                        *dst = out;
                    } else {
                        let bg  = *dst;
                        let src = px & 0x00FFFFFF;
                        let out = alpha_blend(bg, src, a);
                        *dst = out;
                    }
                }
            }
        }
    }
}

fn swap_rb(px: u32) -> u32 {
    let r = (px >> 16) & 0xFF; let g = (px >> 8) & 0xFF; let b = px & 0xFF; let a = (px >> 24) & 0xFF;
    (a << 24) | (b << 16) | (g << 8) | r
}

fn alpha_blend(bg: u32, fg: u32, alpha: u32) -> u32 {
    let blend = |b: u32, f: u32| -> u32 { (f * alpha + b * (255 - alpha)) / 255 };
    let r = blend((bg >> 16) & 0xFF, (fg >> 16) & 0xFF);
    let g = blend((bg >>  8) & 0xFF, (fg >>  8) & 0xFF);
    let b = blend( bg        & 0xFF,  fg        & 0xFF);
    (r << 16) | (g << 8) | b
}

fn fourcc_bpp(fourcc: u32) -> u32 {
    match fourcc {
        0x34325258 | 0x34324258 | 0x34325241 | 0x34324241 => 32,
        0x36313252 | 0x35364742 => 16,
        0x42475258 => 32,
        _ => 32,
    }
}

// ── Global state ──────────────────────────────────────────────────────────

static DEV: Mutex<Option<DrmDevice>> = Mutex::new(None);

pub fn init(fb_phys: u64, w: u32, h: u32, pitch: u32, fmt: u32) {
    let fb_phys = if fb_phys != 0 { fb_phys } else { crate::drivers::gpu::phys_addr() };
    let (w, h, pitch, fmt) = if w > 0 { (w,h,pitch,fmt) } else {
        let (dw,dh) = crate::drivers::gpu::dimensions();
        (dw, dh, crate::drivers::gpu::pitch(), crate::drivers::gpu::format())
    };
    if w == 0 { crate::klog!("DRM: no display"); return; }

    // Map the framebuffer
    let fb_virt = fb_phys + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;

    *DEV.lock() = Some(DrmDevice::new(fb_phys, fb_virt, w, h, pitch, fmt));
    crate::klog!("DRM/KMS: {}x{} display ready, phys={:#x}", w, h, fb_phys);
}

// ── ioctl type encoding ───────────────────────────────────────────────────

fn ioctl_type(req: u64) -> u8  { ((req >> 8) & 0xFF) as u8 }
fn ioctl_nr(req: u64) -> u8    { (req & 0xFF) as u8 }

const DRM_TYPE: u8 = b'd';

// ── DRM ioctl numbers ─────────────────────────────────────────────────────
const NR_VERSION:          u8 = 0x00;
const NR_GET_MAGIC:        u8 = 0x02;
const NR_AUTH_MAGIC:       u8 = 0x11;
const NR_GEM_CLOSE:        u8 = 0x09;
const NR_GEM_FLINK:        u8 = 0x0A;
const NR_GEM_OPEN:         u8 = 0x0B;
const NR_GET_CAP:          u8 = 0x0C;
const NR_SET_CLIENT_CAP:   u8 = 0x0D;
const NR_SET_MASTER:       u8 = 0x1E;
const NR_DROP_MASTER:      u8 = 0x1F;
const NR_PRIME_HANDLE_FD:  u8 = 0x2E;
const NR_PRIME_FD_HANDLE:  u8 = 0x2F;
const NR_MODE_GETRESOURCES:u8 = 0xA0;
const NR_MODE_GETCRTC:     u8 = 0xA1;
const NR_MODE_SETCRTC:     u8 = 0xA2;
const NR_MODE_CURSOR:      u8 = 0xA3;
const NR_MODE_GETGAMMA:    u8 = 0xA4;
const NR_MODE_SETGAMMA:    u8 = 0xA5;
const NR_MODE_GETENCODER:  u8 = 0xA6;
const NR_MODE_GETCONNECTOR:u8 = 0xA7;
const NR_MODE_ADDFB:       u8 = 0xAE;
const NR_MODE_RMFB:        u8 = 0xAF;
const NR_MODE_PAGE_FLIP:   u8 = 0xB0;
const NR_MODE_DIRTYFB:     u8 = 0xB1;
const NR_MODE_CREATE_DUMB: u8 = 0xB2;
const NR_MODE_MAP_DUMB:    u8 = 0xB3;
const NR_MODE_DESTROY_DUMB:u8 = 0xB4;
const NR_MODE_GETPLANES:   u8 = 0xB5;
const NR_MODE_GETPLANE:    u8 = 0xB6;
const NR_MODE_SETPLANE:    u8 = 0xB7;
const NR_MODE_ADDFB2:      u8 = 0xB8;
const NR_MODE_OBJ_GETPROPS:u8 = 0xB9;
const NR_MODE_OBJ_SETPROP: u8 = 0xBA;
const NR_MODE_CURSOR2:     u8 = 0xBB;
const NR_MODE_ATOMIC:      u8 = 0xBC;
const NR_MODE_CREATEBLOB:  u8 = 0xBD;
const NR_MODE_DESTROYBLOB: u8 = 0xBE;
const NR_SYNCOBJ_CREATE:   u8 = 0xBF;
const NR_SYNCOBJ_DESTROY:  u8 = 0xC0;
const NR_SYNCOBJ_FD_WAIT:  u8 = 0xC3;
const NR_SYNCOBJ_RESET:    u8 = 0xC4;
const NR_SYNCOBJ_SIGNAL:   u8 = 0xC5;
const NR_SYNCOBJ_H2FD:     u8 = 0xC1;
const NR_SYNCOBJ_FD2H:     u8 = 0xC2;

/// Main ioctl entry point.
pub fn drm_ioctl(fd: i32, req: u64, arg: u64) -> i64 {
    let t = ioctl_type(req);
    let n = ioctl_nr(req);

    if t != DRM_TYPE { return tty_ioctl(req, arg); }

    match n {
        NR_VERSION           => ioctl_version(arg),
        NR_GET_MAGIC         => { if arg != 0 { unsafe { *(arg as *mut u32) = 0xC0DE; } } 0 }
        NR_AUTH_MAGIC        => 0,
        NR_SET_MASTER        => { DEV.lock().as_mut().map(|d| d.master = true); 0 }
        NR_DROP_MASTER       => { DEV.lock().as_mut().map(|d| d.master = false); 0 }
        NR_GET_CAP           => ioctl_get_cap(arg),
        NR_SET_CLIENT_CAP    => 0,
        NR_GEM_CLOSE         => { if arg != 0 { let h = unsafe { *(arg as *const u32) }; DEV.lock().as_mut().map(|d| d.free_gem(h)); } 0 }
        NR_GEM_FLINK | NR_GEM_OPEN => 0,
        NR_PRIME_HANDLE_FD   => ioctl_prime_h2fd(arg),
        NR_PRIME_FD2H        => ioctl_prime_fd2h(arg),
        NR_MODE_GETRESOURCES => ioctl_get_resources(arg),
        NR_MODE_GETCRTC      => ioctl_get_crtc(arg),
        NR_MODE_SETCRTC      => ioctl_set_crtc(arg),
        NR_MODE_GETENCODER   => ioctl_get_encoder(arg),
        NR_MODE_GETCONNECTOR => ioctl_get_connector(arg),
        NR_MODE_ADDFB        => ioctl_addfb_legacy(arg),
        NR_MODE_ADDFB2       => ioctl_addfb2(arg),
        NR_MODE_RMFB         => { if arg != 0 { let id = unsafe { *(arg as *const u32) }; DEV.lock().as_mut().map(|d| d.fbs.remove(&id)); } 0 }
        NR_MODE_PAGE_FLIP    => ioctl_page_flip(arg),
        NR_MODE_DIRTYFB      => 0,
        NR_MODE_CREATE_DUMB  => ioctl_create_dumb(arg),
        NR_MODE_MAP_DUMB     => ioctl_map_dumb(arg),
        NR_MODE_DESTROY_DUMB => { if arg != 0 { let h = unsafe { *(arg as *const u32) }; DEV.lock().as_mut().map(|d| d.free_gem(h)); } 0 }
        NR_MODE_GETPLANES    => ioctl_get_planes(arg),
        NR_MODE_GETPLANE     => ioctl_get_plane(arg),
        NR_MODE_SETPLANE     => ioctl_set_plane(arg),
        NR_MODE_OBJ_GETPROPS => ioctl_obj_get_props(arg),
        NR_MODE_OBJ_SETPROP  => ioctl_obj_set_prop(arg),
        NR_MODE_CURSOR | NR_MODE_CURSOR2 => ioctl_cursor(arg, n == NR_MODE_CURSOR2),
        NR_MODE_GETGAMMA | NR_MODE_SETGAMMA => 0,
        NR_MODE_ATOMIC       => ioctl_atomic(arg),
        NR_MODE_CREATEBLOB   => ioctl_create_blob(arg),
        NR_MODE_DESTROYBLOB  => { if arg != 0 { let id = unsafe { *(arg as *const u32) }; DEV.lock().as_mut().map(|d| d.blobs.remove(&id)); } 0 }
        NR_SYNCOBJ_CREATE    => ioctl_syncobj_create(arg),
        NR_SYNCOBJ_DESTROY | NR_SYNCOBJ_RESET | NR_SYNCOBJ_SIGNAL => 0,
        NR_SYNCOBJ_FD_WAIT   => 0,
        NR_SYNCOBJ_H2FD | NR_SYNCOBJ_FD2H => 0,
        _ => { crate::klog!("DRM ioctl nr={:#x} req={:#x} unhandled", n, req); 0 }
    }
}

// ── Ioctl implementations ─────────────────────────────────────────────────

#[repr(C)]
struct DrmVersion {
    major: i32, minor: i32, patch: i32,
    name_len: u64, name_ptr: u64,
    date_len: u64, date_ptr: u64,
    desc_len: u64, desc_ptr: u64,
}

fn ioctl_version(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let v = unsafe { &mut *(arg as *mut DrmVersion) };
    v.major = 1; v.minor = 6; v.patch = 0;
    write_len_str(v.name_ptr, &mut v.name_len, b"qunix-drm");
    write_len_str(v.date_ptr, &mut v.date_len, b"20260101");
    write_len_str(v.desc_ptr, &mut v.desc_len, b"Qunix DRM/KMS linear framebuffer");
    0
}

fn write_len_str(ptr: u64, len: &mut u64, s: &[u8]) {
    let max = *len as usize; *len = s.len() as u64;
    if ptr != 0 && max > 0 {
        let n = s.len().min(max.saturating_sub(1));
        unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), ptr as *mut u8, n); *(ptr as *mut u8).add(n) = 0; }
    }
}

#[repr(C)] struct GetCap { cap: u64, val: u64 }

fn ioctl_get_cap(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let c = unsafe { &mut *(arg as *mut GetCap) };
    c.val = match c.cap {
        1  => 1,   // DUMB_BUFFER
        4  => 32,  // DUMB_PREFERRED_DEPTH
        5  => 0,   // DUMB_SHADOW_FB
        6  => 1,   // TIMESTAMP_MONOTONIC
        8  => 64,  // CURSOR_WIDTH
        9  => 64,  // CURSOR_HEIGHT
        10 => 1,   // ADDFB2_MODIFIERS
        13 => 1,   // CRTC_IN_VBLANK_EVENT
        _  => 0,
    };
    0
}

#[repr(C)] struct CardRes {
    fb_ptr: u64, crtc_ptr: u64, conn_ptr: u64, enc_ptr: u64,
    n_fbs: u32, n_crtcs: u32, n_conns: u32, n_encs: u32,
    min_w: u32, max_w: u32, min_h: u32, max_h: u32,
}

fn ioctl_get_resources(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let r = unsafe { &mut *(arg as *mut CardRes) };
    let (w, h) = DEV.lock().as_ref().map(|d| (d.fb_width, d.fb_height)).unwrap_or((1920,1080));

    write_u32_arr(r.crtc_ptr, r.n_crtcs, &[1]);
    write_u32_arr(r.conn_ptr, r.n_conns, &[1]);
    write_u32_arr(r.enc_ptr,  r.n_encs,  &[1]);

    let fb_ids: Vec<u32> = DEV.lock().as_ref().map(|d| d.fbs.keys().copied().collect()).unwrap_or_default();
    write_u32_arr(r.fb_ptr, r.n_fbs, &fb_ids);

    r.n_crtcs = 1; r.n_conns = 1; r.n_encs = 1; r.n_fbs = fb_ids.len() as u32;
    r.min_w = 1; r.max_w = 7680; r.min_h = 1; r.max_h = 4320;
    0
}

fn write_u32_arr(ptr: u64, max: u32, ids: &[u32]) {
    if ptr == 0 || max == 0 { return; }
    let n = ids.len().min(max as usize);
    unsafe { core::ptr::copy_nonoverlapping(ids.as_ptr(), ptr as *mut u32, n); }
}

#[repr(C)] struct ModeCrtc {
    set_conn_ptr: u64, n_conns: u32,
    crtc_id: u32, fb_id: u32, x: u32, y: u32,
    gamma_size: u32, mode_valid: u32,
    mode: ModeInfo,
}

fn ioctl_get_crtc(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let c = unsafe { &mut *(arg as *mut ModeCrtc) };
    let guard = DEV.lock();
    if let Some(d) = guard.as_ref() {
        c.crtc_id    = d.crtc.id;
        c.fb_id      = d.crtc.fb_id;
        c.x          = d.crtc.x;
        c.y          = d.crtc.y;
        c.gamma_size = 256;
        c.mode_valid = d.crtc.mode_valid as u32;
        c.mode       = d.crtc.mode;
    }
    0
}

fn ioctl_set_crtc(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let c = unsafe { &*(arg as *const ModeCrtc) };

    let should_scan = {
        let mut guard = DEV.lock();
        if let Some(d) = guard.as_mut() {
            if c.mode_valid != 0 { d.crtc.mode = c.mode; d.crtc.mode_valid = true; }
            d.crtc.x = c.x; d.crtc.y = c.y;
            if c.fb_id != 0 { d.crtc.fb_id = c.fb_id; }
            d.crtc.active = true;
            d.crtc.fb_id != 0
        } else { false }
    };

    if should_scan {
        let guard = DEV.lock();
        if let Some(d) = guard.as_ref() { d.scanout(); }
    }
    0
}

#[repr(C)] struct ModeEncoder { enc_id: u32, enc_type: u32, crtc_id: u32, possible_crtcs: u32, possible_clones: u32 }

fn ioctl_get_encoder(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let e = unsafe { &mut *(arg as *mut ModeEncoder) };
    e.enc_id = 1; e.enc_type = 2 /*TMDS*/; e.crtc_id = 1; e.possible_crtcs = 1; e.possible_clones = 0;
    0
}

#[repr(C)] struct GetConnector {
    enc_ptr: u64, modes_ptr: u64, props_ptr: u64, pvals_ptr: u64,
    n_modes: u32, n_props: u32, n_encs: u32,
    enc_id: u32, conn_id: u32, conn_type: u32, conn_type_id: u32,
    connection: u32, mm_w: u32, mm_h: u32, subpixel: u32, _pad: u32,
}

fn ioctl_get_connector(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let c = unsafe { &mut *(arg as *mut GetConnector) };
    let (w, h) = DEV.lock().as_ref().map(|d| (d.fb_width, d.fb_height)).unwrap_or((1920,1080));

    let mode = ModeInfo::from_resolution(w, h, 60);
    if c.n_modes >= 1 && c.modes_ptr != 0 { unsafe { *(c.modes_ptr as *mut ModeInfo) = mode; } }
    if c.n_encs  >= 1 && c.enc_ptr   != 0 { unsafe { *(c.enc_ptr as *mut u32) = 1; } }

    c.n_modes = 1; c.n_encs = 1; c.n_props = 0;
    c.enc_id = 1; c.conn_id = 1;
    c.conn_type = 11;    // HDMI-A
    c.conn_type_id = 1;
    c.connection = 1;    // connected
    c.mm_w = (w * 25) / 96;
    c.mm_h = (h * 25) / 96;
    c.subpixel = 1;      // HORIZONTAL_RGB
    0
}

#[repr(C)] struct AddFbLegacy { w: u32, h: u32, pitch: u32, bpp: u32, handle: u32, fb_id: u32 }

fn ioctl_addfb_legacy(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let a = unsafe { &mut *(arg as *mut AddFbLegacy) };
    let mut g = DEV.lock();
    let d = match g.as_mut() { Some(d) => d, None => return -19 };
    let id = d.alloc_fb_id();
    d.fbs.insert(id, DrmFb { id, width: a.w, height: a.h, pitch: a.pitch, bpp: a.bpp, handle: a.handle, format: 0 });
    a.fb_id = id;
    0
}

#[repr(C)] struct AddFb2 {
    w: u32, h: u32, fourcc: u32, flags: u32,
    handles: [u32; 4], pitches: [u32; 4], offsets: [u32; 4],
    modifiers: [u64; 4],
    fb_id: u32, _pad: u32,
}

fn ioctl_addfb2(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let a = unsafe { &mut *(arg as *mut AddFb2) };
    let bpp = fourcc_bpp(a.fourcc);
    let mut g = DEV.lock();
    let d = match g.as_mut() { Some(d) => d, None => return -19 };
    let id = d.alloc_fb_id();
    d.fbs.insert(id, DrmFb { id, width: a.w, height: a.h, pitch: a.pitches[0], bpp, handle: a.handles[0], format: a.fourcc });
    a.fb_id = id;
    0
}

#[repr(C)] struct PageFlip { crtc_id: u32, fb_id: u32, flags: u32, _pad: u32, user_data: u64 }

fn ioctl_page_flip(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let pf = unsafe { &*(arg as *const PageFlip) };
    let should_scan = {
        let mut g = DEV.lock();
        if let Some(d) = g.as_mut() {
            if pf.fb_id != 0 { d.crtc.fb_id = pf.fb_id; }
            d.crtc.active = true;
            d.crtc.fb_id != 0
        } else { false }
    };
    if should_scan { DEV.lock().as_ref().map(|d| d.scanout()); }
    // TODO: deliver vblank event to fd event queue
    0
}

#[repr(C)] struct CreateDumb { h: u32, w: u32, bpp: u32, flags: u32, handle: u32, pitch: u32, size: u64 }

fn ioctl_create_dumb(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let d = unsafe { &mut *(arg as *mut CreateDumb) };
    let mut g = DEV.lock();
    let dev = match g.as_mut() { Some(d) => d, None => return -19 };
    match dev.create_dumb(d.w, d.h, d.bpp) {
        Some(h) => {
            let bo = dev.gems.get(&h).unwrap();
            d.handle = h; d.pitch = bo.pitch; d.size = bo.size; 0
        }
        None => -12,
    }
}

#[repr(C)] struct MapDumb { handle: u32, _pad: u32, offset: u64 }

fn ioctl_map_dumb(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let m = unsafe { &mut *(arg as *mut MapDumb) };
    let g = DEV.lock();
    match g.as_ref().and_then(|d| d.gems.get(&m.handle)) {
        Some(bo) => { m.offset = (m.handle as u64) << 12; 0 }
        None     => -9,
    }
}

#[repr(C)] struct PlaneRes { plane_ptr: u64, n_planes: u32, _pad: u32 }

fn ioctl_get_planes(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let r = unsafe { &mut *(arg as *mut PlaneRes) };
    write_u32_arr(r.plane_ptr, r.n_planes, &[1, 2]); // primary=1, cursor=2
    r.n_planes = 2;
    0
}

#[repr(C)] struct GetPlane {
    plane_id: u32, crtc_id: u32, fb_id: u32, possible_crtcs: u32,
    gamma_size: u32, n_format_types: u32, format_type_ptr: u64,
}

fn ioctl_get_plane(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let p = unsafe { &mut *(arg as *mut GetPlane) };
    let is_cursor = p.plane_id == 2;
    let fb_id = DEV.lock().as_ref().map(|d| d.crtc.fb_id).unwrap_or(0);
    p.crtc_id = 1; p.fb_id = if is_cursor { 0 } else { fb_id };
    p.possible_crtcs = 1; p.gamma_size = 0;
    // Advertise XRGB8888 + XBGR8888 + ARGB8888 + ABGR8888
    let fmts: [u32;4] = [0x34325258, 0x34324258, 0x34325241, 0x34324241];
    if p.n_format_types >= 4 && p.format_type_ptr != 0 {
        unsafe { core::ptr::copy_nonoverlapping(fmts.as_ptr(), p.format_type_ptr as *mut u32, 4); }
    }
    p.n_format_types = 4;
    0
}

#[repr(C)] struct SetPlane {
    plane_id: u32, crtc_id: u32, fb_id: u32, flags: u32,
    cx: i32, cy: i32, cw: u32, ch: u32,
    sx: u32, sy: u32, sh: u32, sw: u32,
}

fn ioctl_set_plane(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let p = unsafe { &*(arg as *const SetPlane) };
    let should_scan = {
        let mut g = DEV.lock();
        if let Some(d) = g.as_mut() {
            if p.plane_id == 2 { // cursor
                d.cursor = PlaneState { fb_id: p.fb_id, crtc_id: p.crtc_id, visible: p.fb_id != 0,
                    crtc_x: p.cx, crtc_y: p.cy, crtc_w: p.cw, crtc_h: p.ch, .. Default::default() };
                if p.fb_id != 0 { d.crtc.cursor_fb = Some(p.fb_id); d.crtc.cursor_x = p.cx; d.crtc.cursor_y = p.cy; }
                else { d.crtc.cursor_fb = None; }
            } else { // primary
                if p.fb_id != 0 { d.crtc.fb_id = p.fb_id; }
                d.primary = PlaneState { fb_id: p.fb_id, crtc_id: p.crtc_id, visible: true,
                    crtc_x: p.cx, crtc_y: p.cy, crtc_w: p.cw, crtc_h: p.ch, .. Default::default() };
            }
            d.crtc.fb_id != 0
        } else { false }
    };
    if should_scan { DEV.lock().as_ref().map(|d| d.scanout()); }
    0
}

// cursor ioctl (legacy drm cursor and cursor2)
#[repr(C)] struct ModeCursor  { flags: u32, crtc_id: u32, x: i32, y: i32, w: u32, h: u32, handle: u32, _pad: u32 }
#[repr(C)] struct ModeCursor2 { flags: u32, crtc_id: u32, x: i32, y: i32, w: u32, h: u32, handle: u32, _pad: u32, hot_x: i32, hot_y: i32 }

fn ioctl_cursor(arg: u64, is_v2: bool) -> i64 {
    if arg == 0 { return 0; }
    let (flags, x, y, w, h, handle) = if is_v2 {
        let c = unsafe { &*(arg as *const ModeCursor2) };
        (c.flags, c.x, c.y, c.w, c.h, c.handle)
    } else {
        let c = unsafe { &*(arg as *const ModeCursor) };
        (c.flags, c.x, c.y, c.w, c.h, c.handle)
    };
    let mut g = DEV.lock();
    if let Some(d) = g.as_mut() {
        d.crtc.cursor_x = x; d.crtc.cursor_y = y;
        if flags & 1 != 0 { // DRM_MODE_CURSOR_BO
            if handle != 0 {
                // Wrap handle in a synthetic FB
                let id = d.alloc_fb_id();
                d.fbs.insert(id, DrmFb { id, width: w, height: h, pitch: w * 4, bpp: 32, handle, format: 0x34325241 });
                d.crtc.cursor_fb = Some(id);
            } else { d.crtc.cursor_fb = None; }
        }
        if flags & 2 != 0 { // DRM_MODE_CURSOR_MOVE
            d.crtc.cursor_x = x; d.crtc.cursor_y = y;
        }
        if d.crtc.fb_id != 0 { d.scanout(); }
    }
    0
}

// Object properties
#[repr(C)] struct ObjGetProps {
    props_ptr: u64, pvals_ptr: u64, n_props: u32, obj_id: u32, obj_type: u32, _pad: u32,
}

fn ioctl_obj_get_props(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    let r = unsafe { &mut *(arg as *mut ObjGetProps) };
    let (props, vals): (&[u32], &[u64]) = match r.obj_type {
        DRM_MODE_OBJECT_CRTC      => (&[PROP_ACTIVE, PROP_MODE_ID, PROP_FB_ID], &[1, 0, 0]),
        DRM_MODE_OBJECT_CONNECTOR => (&[PROP_CRTC_ID], &[1]),
        DRM_MODE_OBJECT_PLANE     => (&[PROP_FB_ID, PROP_CRTC_ID, PROP_SRC_X, PROP_SRC_Y, PROP_SRC_W, PROP_SRC_H,
                                        PROP_CRTC_X, PROP_CRTC_Y, PROP_CRTC_W, PROP_CRTC_H, PROP_TYPE, PROP_IN_FORMATS],
                                      &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0]),
        _ => (&[], &[]),
    };
    let n = props.len().min(r.n_props as usize);
    if r.props_ptr != 0 { unsafe { core::ptr::copy_nonoverlapping(props.as_ptr(), r.props_ptr as *mut u32, n); } }
    if r.pvals_ptr != 0 { unsafe { core::ptr::copy_nonoverlapping(vals.as_ptr(), r.pvals_ptr as *mut u64, n); } }
    r.n_props = props.len() as u32;
    0
}

fn ioctl_obj_set_prop(arg: u64) -> i64 {
    if arg == 0 { return 0; }
    #[repr(C)] struct ObjSetProp { obj_id: u32, obj_type: u32, prop_id: u32, _pad: u32, value: u64 }
    let s = unsafe { &*(arg as *const ObjSetProp) };
    if s.prop_id == PROP_FB_ID && s.value != 0 {
        let fb_id = s.value as u32;
        let should_scan = {
            let mut g = DEV.lock();
            if let Some(d) = g.as_mut() { d.crtc.fb_id = fb_id; d.crtc.fb_id != 0 }
            else { false }
        };
        if should_scan { DEV.lock().as_ref().map(|d| d.scanout()); }
    }
    0
}

// Atomic commit — the modern KMS path
#[repr(C)] struct ModeAtomic {
    flags: u32, n_objs: u32, objs_ptr: u64, n_props_ptr: u64,
    props_ptr: u64, vals_ptr: u64, _reserved: u64, user_data: u64,
}

fn ioctl_atomic(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let a = unsafe { &*(arg as *const ModeAtomic) };
    if a.n_objs == 0 { return 0; }

    let objs  = unsafe { core::slice::from_raw_parts(a.objs_ptr  as *const u32, a.n_objs as usize) };
    let cnts  = unsafe { core::slice::from_raw_parts(a.n_props_ptr as *const u32, a.n_objs as usize) };
    let total = cnts.iter().map(|&c| c as usize).sum::<usize>();
    let props = unsafe { core::slice::from_raw_parts(a.props_ptr as *const u32, total) };
    let vals  = unsafe { core::slice::from_raw_parts(a.vals_ptr  as *const u64, total) };

    let mut pending_fb: Option<u32> = None;
    let mut pending_active = false;
    let mut pending_mode_blob: Option<u32> = None;
    let mut cursor_update = false;

    let mut pi = 0usize;
    for &obj_id in objs.iter() {
        let n = cnts[pi / total.max(1)] as usize; // approximate
        // Walk props for this object
        let obj_props = &props[pi..];
        let obj_vals  = &vals[pi..];
        // Find n_props for this object index
        let obj_idx = objs.iter().position(|&o| o == obj_id).unwrap_or(0);
        let np = cnts.get(obj_idx).copied().unwrap_or(0) as usize;
        for j in 0..np.min(obj_props.len()) {
            let pid = obj_props[j];
            let val = if j < obj_vals.len() { obj_vals[j] } else { 0 };
            match pid {
                p if p == PROP_FB_ID     => { if val != 0 { pending_fb = Some(val as u32); } }
                p if p == PROP_ACTIVE    => { pending_active = val != 0; }
                p if p == PROP_MODE_ID   => { pending_mode_blob = Some(val as u32); }
                p if p == PROP_CRTC_X || p == PROP_CRTC_Y => {}
                _ => {}
            }
        }
        pi += np;
    }

    let should_scan = {
        let mut g = DEV.lock();
        if let Some(d) = g.as_mut() {
            if let Some(fb) = pending_fb { d.crtc.fb_id = fb; }
            if pending_active { d.crtc.active = true; }
            if let Some(blob_id) = pending_mode_blob {
                if let Some(blob_data) = d.blobs.get(&blob_id) {
                    if blob_data.len() >= core::mem::size_of::<ModeInfo>() {
                        let mode = unsafe { *(blob_data.as_ptr() as *const ModeInfo) };
                        d.crtc.mode = mode;
                        d.crtc.mode_valid = true;
                    }
                }
            }
            d.crtc.fb_id != 0 && d.crtc.active
        } else { false }
    };
    if should_scan { DEV.lock().as_ref().map(|d| d.scanout()); }
    0
}

#[repr(C)] struct CreateBlob { data: u64, length: u32, blob_id: u32 }

fn ioctl_create_blob(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let b = unsafe { &mut *(arg as *mut CreateBlob) };
    let data = if b.data != 0 && b.length > 0 {
        unsafe { core::slice::from_raw_parts(b.data as *const u8, b.length as usize).to_vec() }
    } else { Vec::new() };
    let mut g = DEV.lock();
    if let Some(d) = g.as_mut() {
        let id = d.alloc_blob();
        d.blobs.insert(id, data);
        b.blob_id = id;
    }
    0
}

#[repr(C)] struct SyncobjCreate { handle: u32, flags: u32 }
static SYNC_CTR: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

fn ioctl_syncobj_create(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let s = unsafe { &mut *(arg as *mut SyncobjCreate) };
    s.handle = SYNC_CTR.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    0
}

#[repr(C)] struct PrimeH2Fd { handle: u32, flags: u32, fd: i32 }
#[repr(C)] struct PrimeFd2H { fd: i32, flags: u32, handle: u32 }

fn ioctl_prime_h2fd(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let p = unsafe { &mut *(arg as *mut PrimeH2Fd) };
    p.fd = 200 + p.handle as i32; 0
}
fn ioctl_prime_fd2h(arg: u64) -> i64 {
    if arg == 0 { return -22; }
    let p = unsafe { &mut *(arg as *mut PrimeFd2H) };
    p.handle = if p.fd >= 200 { (p.fd - 200) as u32 } else { 1 }; 0
}

// ── TTY ioctls (needed for X11 /dev/tty) ─────────────────────────────────

fn tty_ioctl(req: u64, arg: u64) -> i64 {
    const TCGETS:     u64 = 0x5401;
    const TCSETS:     u64 = 0x5402;
    const TCSETSW:    u64 = 0x5403;
    const TCSETSF:    u64 = 0x5404;
    const TIOCGWINSZ: u64 = 0x5413;
    const TIOCSWINSZ: u64 = 0x5414;
    const TIOCSCTTY:  u64 = 0x540E;
    const TIOCGPGRP:  u64 = 0x540F;
    const TIOCSPGRP:  u64 = 0x5410;
    const TIOCGDEV:   u64 = 0x80045432;
    const FIONREAD:   u64 = 0x541B;
    const FIONBIO:    u64 = 0x5421;
    const FIOCLEX:    u64 = 0x5451;
    const TIOCGPTPEER:u64 = 0x5441;
    const TIOCSTI:    u64 = 0x5412;
    const TIOCNOTTY:  u64 = 0x5422;
    const TIOCGSID:   u64 = 0x5429;

    match req {
        TCGETS | TCSETSW | TCSETSF => {
            if arg != 0 { unsafe { core::ptr::write_bytes(arg as *mut u8, 0, 60); } } 0
        }
        TCSETS => 0,
        TIOCGWINSZ => {
            let (w, h) = DEV.lock().as_ref().map(|d| (d.fb_width, d.fb_height)).unwrap_or((1920,1080));
            let cols = (w / 8).max(80); let rows = (h / 16).max(25);
            if arg != 0 { unsafe {
                *(arg as *mut u16)     = rows as u16;
                *((arg+2) as *mut u16) = cols as u16;
                *((arg+4) as *mut u16) = w as u16;
                *((arg+6) as *mut u16) = h as u16;
            }}
            0
        }
        TIOCSWINSZ => 0,
        TIOCSCTTY  => 0,
        TIOCGPGRP  => { if arg != 0 { unsafe { *(arg as *mut u32) = crate::process::current_pid(); } } 0 }
        TIOCSPGRP  => 0,
        TIOCGDEV   => { if arg != 0 { unsafe { *(arg as *mut u32) = 0x0500; } } 0 }
        FIONREAD   => { if arg != 0 { unsafe { *(arg as *mut i32) = 0; } } 0 }
        FIONBIO | FIOCLEX => 0,
        TIOCGSID   => { if arg != 0 { unsafe { *(arg as *mut u32) = crate::process::current_pid(); } } 0 }
        TIOCNOTTY  => 0,
        _          => 0,
    }
}

// ── Public helpers ────────────────────────────────────────────────────────

/// Map a GEM buffer's physical frames into current process address space.
pub fn mmap_gem(gem_offset: u64, len: u64, prot: crate::memory::vmm::Prot) -> Option<u64> {
    let handle = (gem_offset >> 12) as u32;
    let (bo_phys, bo_size) = {
        let g = DEV.lock();
        let bo = g.as_ref()?.gems.get(&handle)?;
        (bo.phys, bo.size)
    };
    let pages = (len.min(bo_size) + PAGE_SIZE - 1) / PAGE_SIZE;
    let flags = crate::arch::x86_64::paging::PageFlags::PRESENT
        | crate::arch::x86_64::paging::PageFlags::USER
        | crate::arch::x86_64::paging::PageFlags::WRITABLE;
    let mmap_base = crate::process::with_current_mut(|p| {
        let addr = p.address_space.mmap_base;
        p.address_space.mmap_base += pages * PAGE_SIZE + PAGE_SIZE;
        let mut mapper = crate::arch::x86_64::paging::PageMapper::new(p.address_space.pml4_phys);
        for i in 0..pages {
            unsafe { mapper.map_page(addr + i * PAGE_SIZE, bo_phys + i * PAGE_SIZE, flags); }
        }
        p.address_space.regions.push(crate::memory::vmm::VmaRegion {
            start: addr, end: addr + pages * PAGE_SIZE,
            prot, kind: crate::memory::vmm::RegionKind::Device,
            flags: 1, name: alloc::string::String::new(), cow: false,
        });
        addr
    });
    mmap_base
}

pub fn is_ready() -> bool { DEV.lock().as_ref().map(|d| d.fb_width > 0).unwrap_or(false) }
pub fn display_size() -> (u32, u32) { DEV.lock().as_ref().map(|d| (d.fb_width, d.fb_height)).unwrap_or((0,0)) }
