//! Qunix kernel RTOS primitives — SpinLock with backoff, Semaphore, RealTimeClock.
//! SpinLock here adds exponential backoff; use spin::Mutex for the simple case.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use alloc::collections::VecDeque;
use spin::Mutex;
use crate::process::Pid;

// ── Fast spinlock with exponential backoff ──────────────────────────────────

pub struct SpinLock(AtomicBool);

impl SpinLock {
    pub const fn new() -> Self { SpinLock(AtomicBool::new(false)) }

    pub fn lock(&self) {
        let mut backoff = 1u32;
        while self.0.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            for _ in 0..backoff { core::hint::spin_loop(); }
            backoff = (backoff * 2).min(128);
        }
    }

    pub fn unlock(&self) { self.0.store(false, Ordering::Release); }

    pub fn try_lock(&self) -> bool {
        self.0.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok()
    }
}

// ── Counting semaphore ──────────────────────────────────────────────────────

pub struct Semaphore {
    count:   AtomicU32,
    waiters: Mutex<VecDeque<Pid>>,
}

impl Semaphore {
    pub const fn new(n: u32) -> Self {
        Semaphore { count: AtomicU32::new(n), waiters: Mutex::new(VecDeque::new()) }
    }

    pub fn wait(&self) {
        loop {
            let c = self.count.load(Ordering::Acquire);
            if c > 0 {
                if self.count.compare_exchange(c, c - 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                    return;
                }
            } else {
                let pid = crate::process::current_pid();
                self.waiters.lock().push_back(pid);
                crate::sched::block_current(crate::process::ProcessState::Sleeping);
            }
        }
    }

    pub fn signal(&self) {
        self.count.fetch_add(1, Ordering::Release);
        if let Some(pid) = self.waiters.lock().pop_front() {
            crate::sched::wake_process(pid);
        }
    }

    pub fn try_wait(&self) -> bool {
        loop {
            let c = self.count.load(Ordering::Acquire);
            if c == 0 { return false; }
            if self.count.compare_exchange(c, c - 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                return true;
            }
        }
    }
}

// ── Wait queue ────────────────────────────────────────────────────────────

pub struct WaitQueue(Mutex<VecDeque<Pid>>);

impl WaitQueue {
    pub const fn new() -> Self { WaitQueue(Mutex::new(VecDeque::new())) }

    pub fn wait(&self) {
        let pid = crate::process::current_pid();
        self.0.lock().push_back(pid);
        crate::sched::block_current(crate::process::ProcessState::Sleeping);
    }

    pub fn wake_one(&self) {
        if let Some(pid) = self.0.lock().pop_front() {
            crate::sched::wake_process(pid);
        }
    }

    pub fn wake_all(&self) {
        let pids: alloc::vec::Vec<Pid> = self.0.lock().drain(..).collect();
        for pid in pids { crate::sched::wake_process(pid); }
    }
}

// ── Mutex (blocking) ────────────────────────────────────────────────────────

pub struct KMutex {
    locked:  AtomicBool,
    owner:   AtomicU32,
    waiters: Mutex<VecDeque<Pid>>,
}

impl KMutex {
    pub const fn new() -> Self {
        KMutex {
            locked:  AtomicBool::new(false),
            owner:   AtomicU32::new(0),
            waiters: Mutex::new(VecDeque::new()),
        }
    }

    pub fn lock(&self) {
        let pid = crate::process::current_pid();
        loop {
            if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                self.owner.store(pid, Ordering::Relaxed);
                return;
            }
            self.waiters.lock().push_back(pid);
            crate::sched::block_current(crate::process::ProcessState::Sleeping);
        }
    }

    pub fn unlock(&self) {
        self.owner.store(0, Ordering::Relaxed);
        self.locked.store(false, Ordering::Release);
        if let Some(pid) = self.waiters.lock().pop_front() {
            crate::sched::wake_process(pid);
        }
    }

    pub fn try_lock(&self) -> bool {
        if self.locked.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            self.owner.store(crate::process::current_pid(), Ordering::Relaxed);
            true
        } else {
            false
        }
    }
}

// ── RCU epoch counter (simplified) ─────────────────────────────────────────

static RCU_EPOCH: AtomicU32 = AtomicU32::new(0);

pub fn rcu_read_lock() -> u32 { RCU_EPOCH.load(Ordering::Acquire) }
pub fn rcu_read_unlock(_epoch: u32) {}
pub fn rcu_synchronize() { RCU_EPOCH.fetch_add(1, Ordering::AcqRel); }
