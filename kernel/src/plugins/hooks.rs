//! Hook call sites — thin wrappers that check whether any plugin is active
//! before entering the full dispatch loop.
//!
//! A global `HOOKS_ACTIVE` counter tracks how many plugins are enabled.
//! When it is zero, all hook call sites reduce to a single atomic load + branch
//! — essentially free.

use core::sync::atomic::{AtomicU32, Ordering};
use super::*;

/// Number of currently-enabled plugins. Maintained by enable/disable.
pub static HOOKS_ACTIVE: AtomicU32 = AtomicU32::new(0);

/// Fast-path guard: returns true only if at least one plugin is active.
#[inline(always)]
pub fn any_active() -> bool {
    HOOKS_ACTIVE.load(Ordering::Relaxed) > 0
}

/// Call from syscall dispatch entry — before handler is invoked.
#[inline]
pub fn pre_syscall(nr: u64, pid: u32, a0: u64, a1: u64, a2: u64) {
    if !any_active() { return; }
    dispatch_pre_syscall(&SyscallCtx { nr, pid, arg0: a0, arg1: a1, arg2: a2 });
}

/// Call from syscall dispatch exit — after handler returns.
#[inline]
pub fn post_syscall(nr: u64, pid: u32, retval: i64) {
    if !any_active() { return; }
    dispatch_post_syscall(
        &SyscallCtx { nr, pid, arg0: 0, arg1: 0, arg2: 0 },
        retval,
    );
}

/// Call from scheduler tick (timer interrupt, 1 kHz).
#[inline]
pub fn scheduler_tick(cpu: u32, prev: u32, next: u32, tick: u64) {
    if !any_active() { return; }
    dispatch_scheduler_tick(&SchedCtx {
        cpu, prev_pid: prev, next_pid: next, tick_count: tick,
    });
}

/// Call from network receive path.
#[inline]
pub fn net_packet_in(data: &[u8], src_ip: u32, dst_ip: u32, proto: u8) {
    if !any_active() { return; }
    dispatch_net_packet_in(&NetCtx { data, src_ip, dst_ip, protocol: proto });
}

/// Call from VFS open/read/write/close/unlink paths.
#[inline]
pub fn fs_operation(op: &str, path: &str, pid: u32) {
    if !any_active() { return; }
    dispatch_fs_operation(&FsOpCtx { op, path, pid });
}

/// Call from driver event paths.
#[inline]
pub fn driver_event(driver: &str, event: &str, data: u64) {
    if !any_active() { return; }
    dispatch_driver_event(&DriverEventCtx { driver, event, data });
}
