/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

pub mod abi;
pub mod drm;
pub mod input;
pub mod syscall;

pub fn init() {
    crate::klog!("Qunix ABI compatibility layer ready");
    syscall::init();
    // Auto-load DRM module
    let _ = crate::module::load("drm_qunix");
}

/// Personality 0 = POSIX/Linux userland ABI (default for exec'd binaries)
pub const PERSONALITY_POSIX: u32 = 0x0000;
/// Kept for API compatibility — same value as PERSONALITY_POSIX
pub const PERSONALITY_LINUX: u32 = 0x0000;
pub const PERSONALITY_QUNIX: u32 = 0xFFFF;

pub fn set_personality_posix() {
    crate::process::with_current_mut(|p| p.personality = PERSONALITY_POSIX);
}
/// Alias kept for existing callers
pub fn set_personality_linux() { set_personality_posix(); }

pub fn set_personality_qunix() {
    crate::process::with_current_mut(|p| p.personality = PERSONALITY_QUNIX);
}
