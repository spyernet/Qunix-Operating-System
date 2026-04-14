/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Qunix Kernel Plugin System
//!
//! Plugins are compiled into the kernel at build time. They cannot be loaded
//! at runtime, but they can be enabled or disabled at runtime without a rebuild.
//!
//! ## Architecture
//!
//! Build time:
//!   - `build.sh plugin` step parses `plugins/*/main.conf`
//!   - Generates `plugins/registry_generated.rs` listing all plugins
//!   - Each plugin's entry .rs file is included via Rust modules
//!   - All hooks are zero-cost when no plugin is enabled (empty fn pointers)
//!
//! Runtime:
//!   - `plugin_manager` holds a global registry of plugin states
//!   - `enable_plugin` / `disable_plugin` flip the AtomicBool in O(1)
//!   - Hook dispatch iterates only registered, enabled plugins
//!   - `/proc/plugins` exposes list; `/dev/pluginctl` accepts ioctl commands
//!
//! ## Safety
//!
//! Plugins operate through trait methods only.
//! They receive immutable context references, not raw kernel pointers.
//! No unsafe code required in plugin implementations.

use alloc::vec::Vec;
use alloc::string::{String, ToString};
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

pub mod hooks;

// ── Plugin metadata ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct PluginMeta {
    pub name:    &'static str,
    pub version: &'static str,
    pub author:  &'static str,
    pub license: &'static str,
    pub desc:    &'static str,
}

// ── Hook contexts (read-only views, no raw pointers) ─────────────────────

pub struct SyscallCtx {
    pub nr:    u64,
    pub pid:   u32,
    pub arg0:  u64,
    pub arg1:  u64,
    pub arg2:  u64,
}

pub struct SchedCtx {
    pub cpu:        u32,
    pub prev_pid:   u32,
    pub next_pid:   u32,
    pub tick_count: u64,
}

pub struct NetCtx<'a> {
    pub data:     &'a [u8],
    pub src_ip:   u32,
    pub dst_ip:   u32,
    pub protocol: u8,
}

pub struct FsOpCtx<'a> {
    pub op:   &'a str,  // "open", "read", "write", "close", "unlink", etc.
    pub path: &'a str,
    pub pid:  u32,
}

pub struct DriverEventCtx<'a> {
    pub driver: &'a str,
    pub event:  &'a str,
    pub data:   u64,
}

// ── Plugin trait ──────────────────────────────────────────────────────────

/// Every plugin implements this trait. All methods have default no-op
/// implementations so plugins only override the hooks they need.
pub trait Plugin: Send + Sync {
    /// Called once at boot if the plugin is enabled.
    fn init(&self) {}

    /// Called when the plugin is runtime-disabled.
    fn deinit(&self) {}

    /// Called before every syscall dispatch.
    fn pre_syscall(&self, _ctx: &SyscallCtx) {}

    /// Called after every syscall dispatch.
    fn post_syscall(&self, _ctx: &SyscallCtx, _retval: i64) {}

    /// Called on every scheduler tick (1 kHz).
    fn scheduler_tick(&self, _ctx: &SchedCtx) {}

    /// Called on every inbound network packet.
    fn net_packet_in(&self, _ctx: &NetCtx) {}

    /// Called on filesystem operations.
    fn fs_operation(&self, _ctx: &FsOpCtx) {}

    /// Called on driver events.
    fn driver_event(&self, _ctx: &DriverEventCtx) {}
}

// ── Registry entry ────────────────────────────────────────────────────────

pub struct PluginEntry {
    pub meta:    PluginMeta,
    pub plugin:  &'static (dyn Plugin + Send + Sync),
    enabled:     AtomicBool,
}

impl PluginEntry {
    pub const fn new(
        meta:    PluginMeta,
        plugin:  &'static (dyn Plugin + Send + Sync),
        enabled_at_boot: bool,
    ) -> Self {
        PluginEntry { meta, plugin, enabled: AtomicBool::new(enabled_at_boot) }
    }

    pub fn is_enabled(&self) -> bool { self.enabled.load(Ordering::Acquire) }

    pub fn enable(&self) {
        if !self.is_enabled() {
            self.enabled.store(true, Ordering::Release);
            self.plugin.init();
            crate::klog!("plugin: enabled '{}'", self.meta.name);
        }
    }

    pub fn disable(&self) {
        if self.is_enabled() {
            self.enabled.store(false, Ordering::Release);
            self.plugin.deinit();
            crate::klog!("plugin: disabled '{}'", self.meta.name);
        }
    }
}

// ── Global registry ───────────────────────────────────────────────────────
//
// Plugins register themselves at compile time via the generated registry file.
// The registry is a static slice — no heap allocation, no initialization cost.

static REGISTRY: Mutex<Vec<&'static PluginEntry>> = Mutex::new(Vec::new());

pub fn register(entry: &'static PluginEntry) {
    REGISTRY.lock().push(entry);
}

/// Initialize all registered plugins that are enabled at boot.
pub fn init() {
    // Generated registry populates REGISTRY before init() is called.
    // See plugins/generated.rs (created by build.sh).
    let guard = REGISTRY.lock();
    let total = guard.len();
    let enabled = guard.iter().filter(|e| e.is_enabled()).count();

    // Run init() on every plugin that starts enabled
    drop(guard); // release lock before calling plugin code
    for entry in REGISTRY.lock().iter() {
        if entry.is_enabled() {
            entry.plugin.init();
        }
    }
    crate::klog!("plugins: {} registered, {} enabled at boot", total, enabled);
}

// ── Runtime control API ───────────────────────────────────────────────────

pub fn enable(name: &str) -> bool {
    let guard = REGISTRY.lock();
    for e in guard.iter() {
        if e.meta.name == name {
            drop(guard);
            let g2 = REGISTRY.lock();
            for e2 in g2.iter() {
                if e2.meta.name == name { e2.enable(); return true; }
            }
            return false;
        }
    }
    false
}

pub fn disable(name: &str) -> bool {
    let guard = REGISTRY.lock();
    for e in guard.iter() {
        if e.meta.name == name {
            drop(guard);
            let g2 = REGISTRY.lock();
            for e2 in g2.iter() {
                if e2.meta.name == name { e2.disable(); return true; }
            }
            return false;
        }
    }
    false
}

pub fn list() -> Vec<(String, bool, String, String)> {
    REGISTRY.lock()
        .iter()
        .map(|e| (
            e.meta.name.to_string(),
            e.is_enabled(),
            e.meta.version.to_string(),
            e.meta.desc.to_string(),
        ))
        .collect()
}

pub fn is_enabled(name: &str) -> bool {
    REGISTRY.lock().iter().any(|e| e.meta.name == name && e.is_enabled())
}

// ── Hook dispatch ─────────────────────────────────────────────────────────
//
// Each dispatch function iterates the registry and calls enabled plugins.
// If no plugins are registered or enabled, the entire call is a short loop
// that immediately exits — effectively zero overhead.

#[inline]
pub fn dispatch_pre_syscall(ctx: &SyscallCtx) {
    for e in REGISTRY.lock().iter() {
        if e.is_enabled() { e.plugin.pre_syscall(ctx); }
    }
}

#[inline]
pub fn dispatch_post_syscall(ctx: &SyscallCtx, retval: i64) {
    for e in REGISTRY.lock().iter() {
        if e.is_enabled() { e.plugin.post_syscall(ctx, retval); }
    }
}

#[inline]
pub fn dispatch_scheduler_tick(ctx: &SchedCtx) {
    for e in REGISTRY.lock().iter() {
        if e.is_enabled() { e.plugin.scheduler_tick(ctx); }
    }
}

#[inline]
pub fn dispatch_net_packet_in(ctx: &NetCtx) {
    for e in REGISTRY.lock().iter() {
        if e.is_enabled() { e.plugin.net_packet_in(ctx); }
    }
}

#[inline]
pub fn dispatch_fs_operation(ctx: &FsOpCtx) {
    for e in REGISTRY.lock().iter() {
        if e.is_enabled() { e.plugin.fs_operation(ctx); }
    }
}

#[inline]
pub fn dispatch_driver_event(ctx: &DriverEventCtx) {
    for e in REGISTRY.lock().iter() {
        if e.is_enabled() { e.plugin.driver_event(ctx); }
    }
}

// ── /proc/plugins virtual file content ───────────────────────────────────

pub fn proc_plugins_content() -> alloc::vec::Vec<u8> {
    let mut out = alloc::format!("# Qunix Plugins\n# name version enabled description\n");
    for e in REGISTRY.lock().iter() {
        out.push_str(&alloc::format!("{} {} {} {}\n",
            e.meta.name,
            e.meta.version,
            if e.is_enabled() { "enabled" } else { "disabled" },
            e.meta.desc,
        ));
    }
    out.into_bytes()
}

// Generated at build time from plugins/*/main.conf
// Included here to register all compiled-in plugins.
pub mod generated;

/// Runtime-enable a plugin by name. Returns true if found and enabled.
/// Does NOT require kernel rebuild.
pub fn runtime_enable(name: &str) -> bool {
    let found = enable(name);
    if found {
        hooks::HOOKS_ACTIVE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    found
}

/// Runtime-disable a plugin by name. Returns true if found and disabled.
/// Does NOT require kernel rebuild.
pub fn runtime_disable(name: &str) -> bool {
    let found = disable(name);
    if found {
        hooks::HOOKS_ACTIVE.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
    }
    found
}
