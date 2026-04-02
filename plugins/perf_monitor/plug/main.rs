// perf_monitor — tracks scheduler ticks and context switches per CPU.
//
// Exposes data via the kernel log on disable and via the pre_syscall hook
// when a process reads /proc/plugins_perf.
//
// Runtime toggle via: pluginctl enable/disable perf_monitor

use crate::plugins::{Plugin, SchedCtx, SyscallCtx, PluginMeta, PluginEntry};
use core::sync::atomic::{AtomicU64, Ordering};

// One counter set per CPU (indexed 0..63)
const MAX_CPUS: usize = 64;

static TICK_COUNTS:    [AtomicU64; MAX_CPUS] = {
    [const { AtomicU64::new(0) }; MAX_CPUS]
};
static SWITCH_COUNTS:  [AtomicU64; MAX_CPUS] = {
    [const { AtomicU64::new(0) }; MAX_CPUS]
};
static TOTAL_SYSCALLS: AtomicU64 = AtomicU64::new(0);

pub struct PerfMonitor;

impl Plugin for PerfMonitor {
    fn init(&self) {
        // Reset counters on (re)enable
        for i in 0..MAX_CPUS {
            TICK_COUNTS[i].store(0, Ordering::Relaxed);
            SWITCH_COUNTS[i].store(0, Ordering::Relaxed);
        }
        TOTAL_SYSCALLS.store(0, Ordering::Relaxed);
        crate::klog!("perf_monitor: active — tracking scheduler + syscall load");
    }

    fn deinit(&self) {
        // Print summary to kernel log when disabled
        let ticks:   u64 = TICK_COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).sum();
        let switches:u64 = SWITCH_COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).sum();
        let syscalls:u64 = TOTAL_SYSCALLS.load(Ordering::Relaxed);
        crate::klog!("perf_monitor: summary ticks={} ctx_switches={} syscalls={}",
            ticks, switches, syscalls);
    }

    fn scheduler_tick(&self, ctx: &SchedCtx) {
        let cpu = (ctx.cpu as usize).min(MAX_CPUS - 1);
        TICK_COUNTS[cpu].fetch_add(1, Ordering::Relaxed);
        if ctx.prev_pid != ctx.next_pid {
            SWITCH_COUNTS[cpu].fetch_add(1, Ordering::Relaxed);
        }
    }

    fn pre_syscall(&self, _ctx: &SyscallCtx) {
        TOTAL_SYSCALLS.fetch_add(1, Ordering::Relaxed);
    }
}

// ── Snapshot for /proc/plugins_perf ──────────────────────────────────────

pub fn snapshot() -> (u64, u64, u64) {
    let ticks:   u64 = TICK_COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).sum();
    let switches:u64 = SWITCH_COUNTS.iter().map(|c| c.load(Ordering::Relaxed)).sum();
    let syscalls:u64 = TOTAL_SYSCALLS.load(Ordering::Relaxed);
    (ticks, switches, syscalls)
}

// ── Static plugin instance ────────────────────────────────────────────────

static INSTANCE: PerfMonitor = PerfMonitor;

pub static PLUGIN_ENTRY: PluginEntry = PluginEntry::new(
    PluginMeta {
        name:    "perf_monitor",
        version: "1.0",
        author:  "Qunix Contributors",
        license: "MIT",
        desc:    "Tracks scheduler ticks and context switches per CPU",
    },
    &INSTANCE,
    true, // enabled at boot by default
);
