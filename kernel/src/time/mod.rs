/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Time subsystem — PIT timer, monotonic clock, real-time clock, sleep.

use core::sync::atomic::{AtomicU64, AtomicI64, Ordering};
use crate::arch::x86_64::port::{outb, inb};

const PIT_FREQ:      u64 = 1193182;
const TICKS_PER_SEC: u64 = 1000;
const PIT_DIVISOR:   u16 = (PIT_FREQ / TICKS_PER_SEC) as u16;

static MONOTONIC_TICKS: AtomicU64 = AtomicU64::new(0);
static REALTIME_OFFSET: AtomicI64 = AtomicI64::new(0); // seconds since epoch at tick 0

pub fn init() {
    unsafe {
        // PIT channel 0, mode 3 (square wave), 16-bit latch
        outb(0x43, 0x36);
        outb(0x40, (PIT_DIVISOR & 0xFF) as u8);
        outb(0x40, (PIT_DIVISOR >> 8) as u8);
    }
    // Seed real-time from CMOS RTC
    let epoch = read_rtc_epoch();
    REALTIME_OFFSET.store(epoch, Ordering::Relaxed);

    crate::drivers::irq::register(0, timer_irq);
    crate::klog!("Time: PIT @ {}Hz, RTC epoch={}", TICKS_PER_SEC, epoch);
}

fn timer_irq(_: &crate::arch::x86_64::interrupts::InterruptFrame) {
    let t = MONOTONIC_TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::tty::poll_input_devices();
    wake_sleeping_processes(t);
    crate::sched::tick();
    // In IRQ context: only deliver default-disposition signals.
    // User-handler signals stay pending until syscall exit.
    crate::signal::dispatch_pending_from_irq(crate::process::current_pid());
}

pub fn wake_sleeping_processes(now: u64) {
    for pid in crate::process::all_pids() {
        let needs_wake = crate::process::with_process(pid, |p|
            p.state == crate::process::ProcessState::Sleeping
                && p.sleep_until != 0
                && p.sleep_until <= now
        ).unwrap_or(false);

        if needs_wake {
            // Clear sleep_until first so timer doesn't fire again on next tick.
            // IMPORTANT: do NOT change state here — wake_process does the
            // Sleeping→Runnable transition atomically with the enqueue.
            // Setting state=Runnable here before wake_process would cause
            // wake_process to see a non-Sleeping state and skip the enqueue,
            // leaving the process as Runnable but not in the run queue.
            crate::process::with_process_mut(pid, |p| {
                p.sleep_until = 0;
                // Leave p.state = Sleeping so wake_process transitions it properly
            });
            crate::sched::wake_process(pid);  // Sleeping → Runnable + enqueue
        }
    }
}

// ── Clock accessors ──────────────────────────────────────────────────────

/// Monotonic tick count (1 ms per tick).
pub fn ticks() -> u64 { MONOTONIC_TICKS.load(Ordering::Relaxed) }

/// Uptime in milliseconds.
pub fn uptime_ms() -> u64 { ticks() }

/// Monotonic time in nanoseconds.
pub fn monotonic_ns() -> u64 { ticks() * 1_000_000 }

/// Wall clock seconds since Unix epoch.
pub fn realtime_secs() -> i64 {
    REALTIME_OFFSET.load(Ordering::Relaxed) + (ticks() / 1000) as i64
}

/// Wall clock in (seconds, nanoseconds).
pub fn realtime() -> (i64, i64) {
    let t    = ticks();
    let secs = REALTIME_OFFSET.load(Ordering::Relaxed) + (t / 1000) as i64;
    let ns   = ((t % 1000) * 1_000_000) as i64;
    (secs, ns)
}

/// Set the real-time offset (used by settimeofday / clock_settime).
pub fn set_realtime(epoch_secs: i64) {
    let t = (ticks() / 1000) as i64;
    REALTIME_OFFSET.store(epoch_secs - t, Ordering::Relaxed);
}

// ── Sleep helpers ────────────────────────────────────────────────────────

/// Block current process for `ms` milliseconds.
pub fn sleep_ms(ms: u64) {
    if ms == 0 { return; }
    let wake_at = ticks() + ms;
    let pid = crate::process::current_pid();
    crate::process::with_process_mut(pid, |p| {
        p.sleep_until = wake_at;
        p.state = crate::process::ProcessState::Sleeping;
    });
    // Remove from run queue — tick() will re-enqueue when deadline passes
    crate::sched::remove_task(pid);
    crate::sched::schedule_next_from_irq();
}

/// Block for `ticks` timer ticks.
pub fn sleep_ticks(t: u64) { sleep_ms(t); }

/// Busy-wait for `us` microseconds (short delays only, ≤1000 µs).
pub fn udelay(us: u64) {
    let start = ticks();
    let need  = us / 1000 + 1;
    while ticks() - start < need { core::hint::spin_loop(); }
}

// ── CMOS RTC ─────────────────────────────────────────────────────────────

fn cmos_read(reg: u8) -> u8 {
    unsafe { outb(0x70, reg); inb(0x71) }
}

fn bcd_to_bin(v: u8) -> u8 { (v & 0x0F) + (v >> 4) * 10 }

fn days_in_month(m: u32, y: u32) -> u32 {
    match m {
        1|3|5|7|8|10|12 => 31,
        4|6|9|11        => 30,
        2 => if y % 400 == 0 || (y % 4 == 0 && y % 100 != 0) { 29 } else { 28 },
        _               => 30,
    }
}

fn is_leap(y: u32) -> bool { y % 400 == 0 || (y % 4 == 0 && y % 100 != 0) }

fn date_to_epoch(year: u32, month: u32, day: u32, h: u32, m: u32, s: u32) -> i64 {
    // Days since 1970-01-01
    let mut days = 0i64;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    for mon in 1..month {
        days += days_in_month(mon, year) as i64;
    }
    days += (day - 1) as i64;
    days * 86400 + h as i64 * 3600 + m as i64 * 60 + s as i64
}

fn read_rtc_epoch() -> i64 {
    // Wait for no update in progress
    for _ in 0..1000 {
        if cmos_read(0x0A) & 0x80 == 0 { break; }
    }

    let status_b = cmos_read(0x0B);
    let is_bcd   = status_b & 0x04 == 0;
    let is_24h   = status_b & 0x02 != 0;

    let conv = |v: u8| if is_bcd { bcd_to_bin(v) } else { v };

    let sec   = conv(cmos_read(0x00)) as u32;
    let min   = conv(cmos_read(0x02)) as u32;
    let mut h = conv(cmos_read(0x04)) as u32;
    let day   = conv(cmos_read(0x07)) as u32;
    let month = conv(cmos_read(0x08)) as u32;
    let year  = conv(cmos_read(0x09)) as u32 + 2000;

    if !is_24h && cmos_read(0x04) & 0x80 != 0 {
        h = (h & 0x7F) + 12;
    }

    if month == 0 || day == 0 {
        return 1_700_000_000; // Fallback: ~2023
    }

    date_to_epoch(year, month, day, h, min, sec)
}

// ── POSIX timespec helpers ───────────────────────────────────────────────

#[repr(C)]
pub struct Timespec { pub tv_sec: i64, pub tv_nsec: i64 }

impl Timespec {
    pub fn to_ms(&self) -> u64 {
        (self.tv_sec as u64) * 1000 + (self.tv_nsec as u64) / 1_000_000
    }
}

pub fn clock_gettime(clk_id: i32) -> Timespec {
    match clk_id {
        0 => { let (s, n) = realtime();     Timespec { tv_sec: s, tv_nsec: n } }
        1 => { let t = monotonic_ns() as i64; Timespec { tv_sec: t/1_000_000_000, tv_nsec: t%1_000_000_000 } }
        _ => { let t = monotonic_ns() as i64; Timespec { tv_sec: t/1_000_000_000, tv_nsec: t%1_000_000_000 } }
    }
}
