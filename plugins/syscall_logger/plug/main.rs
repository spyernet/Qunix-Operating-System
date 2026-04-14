/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

// syscall_logger — logs every syscall number and issuing PID.
//
// This plugin hooks pre_syscall and writes a compact entry to the kernel
// serial log. Useful for tracing, debugging, and security auditing.
//
// Runtime toggle via: pluginctl enable/disable syscall_logger

use crate::plugins::{Plugin, SyscallCtx, PluginMeta, PluginEntry};
use core::sync::atomic::{AtomicU64, Ordering};

static CALL_COUNT: AtomicU64 = AtomicU64::new(0);

// Common syscall names for readable output
fn syscall_name(nr: u64) -> &'static str {
    match nr {
        0  => "read",     1  => "write",    2  => "open",
        3  => "close",    4  => "stat",     5  => "fstat",
        8  => "lseek",    9  => "mmap",     11 => "munmap",
        12 => "brk",      39 => "getpid",   56 => "clone",
        57 => "fork",     59 => "execve",   60 => "exit",
        61 => "wait4",    62 => "kill",     89 => "readlink",
        102=> "getuid",   104=> "getgid",   158=> "arch_prctl",
        201=> "clock_gettime", 231=> "exit_group",
        257=> "openat",   _  => "syscall",
    }
}

pub struct SyscallLogger;

impl Plugin for SyscallLogger {
    fn init(&self) {
        crate::klog!("syscall_logger: active — logging all syscalls");
        CALL_COUNT.store(0, Ordering::Relaxed);
    }

    fn deinit(&self) {
        let n = CALL_COUNT.load(Ordering::Relaxed);
        crate::klog!("syscall_logger: disabled — logged {} calls total", n);
    }

    fn pre_syscall(&self, ctx: &SyscallCtx) {
        // Only log non-trivial syscalls to avoid flooding the log.
        // Skip high-frequency: clock_gettime(228), getpid(39)
        if ctx.nr == 228 || ctx.nr == 39 { return; }

        CALL_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::klog!("[syscall_logger] pid={} nr={}({})",
            ctx.pid, ctx.nr, syscall_name(ctx.nr));
    }
}

// ── Static plugin instance ────────────────────────────────────────────────

static INSTANCE: SyscallLogger = SyscallLogger;

pub static PLUGIN_ENTRY: PluginEntry = PluginEntry::new(
    PluginMeta {
        name:    "syscall_logger",
        version: "1.0",
        author:  "Qunix Contributors",
        license: "MIT",
        desc:    "Logs syscall numbers and PIDs via kernel debug output",
    },
    &INSTANCE,
    false, // disabled at boot by default to avoid flooding console
);
