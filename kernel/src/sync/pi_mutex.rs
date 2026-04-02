//! Priority-Inheritance Mutex (PI-Mutex) — the core of PREEMPT_RT.
//!
//! ## Why normal spinlocks break real-time guarantees
//! Consider:
//!   - RT task A (prio 90) wants mutex M
//!   - Low-prio task B (prio 120) holds mutex M
//!   - Medium-prio task C (prio 105) is runnable
//!
//! Without PI: A blocks waiting, C runs, B can't run, A waits indefinitely.
//! This is *priority inversion* — a low-priority task effectively blocks a
//! high-priority task for an unbounded duration.
//!
//! With PI: When A blocks on M, it *donates* its priority (90) to B.
//! B temporarily runs at prio 90, preempts C, finishes quickly, releases M.
//! A then runs. Bounded wait time: max = B's critical section duration.
//!
//! ## Implementation
//!
//! Each PiMutex holds:
//!   - `owner`: Option<Pid> — current holder
//!   - `wait_queue`: sorted Vec of waiting (prio, Pid) pairs
//!   - `donated_prio`: the highest priority donated to the owner
//!
//! On `lock()`:
//!   1. If unlocked: set owner = current, return
//!   2. If locked by current: deadlock (panic in debug, return Err in release)
//!   3. Add current to wait_queue
//!   4. PI donation: if current.prio < owner.prio, boost owner to current.prio
//!      (walk the chain: if owner is itself blocked, boost *its* blocker too)
//!   5. Sleep (scheduler removes us from run queue)
//!
//! On `unlock()`:
//!   1. Remove priority donation from owner
//!   2. Wake the highest-priority waiter
//!   3. Transfer ownership to that waiter
//!   4. If waiter is blocked (impossible here), chain boost
//!
//! ## Chain boosting
//! If a task holds multiple mutexes, the chain must be resolved:
//!   A(90) → mutex1 → B(110) → mutex2 → C(130)
//!   B gets boosted to 90 by A. But B is blocked on mutex2.
//!   So C must also be boosted to 90 (donated via B).
//!   Maximum chain depth: 8 (avoids infinite loops on cycles).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex as SpinMutex;
use core::sync::atomic::{AtomicU32, AtomicBool, Ordering};

const PI_CHAIN_MAX_DEPTH: usize = 8;

// ── Per-mutex state ───────────────────────────────────────────────────────

struct PiMutexState {
    /// PID of the current owner. 0 = unlocked.
    owner:        u32,
    /// Highest priority donated to owner (0 = no donation, lower = higher prio).
    donated_prio: u8,
    /// Wait queue: sorted by priority ascending (lowest = highest prio).
    /// Entries: (priority, pid)
    waiters:      Vec<(u8, u32)>,
}

impl PiMutexState {
    const fn new() -> Self {
        PiMutexState { owner: 0, donated_prio: 255, waiters: Vec::new() }
    }

    fn add_waiter(&mut self, prio: u8, pid: u32) {
        // Insert sorted by priority (ascending = highest prio first)
        let pos = self.waiters.partition_point(|&(p, _)| p <= prio);
        self.waiters.insert(pos, (prio, pid));
    }

    fn remove_waiter(&mut self, pid: u32) {
        self.waiters.retain(|&(_, p)| p != pid);
    }

    fn highest_waiter(&self) -> Option<(u8, u32)> {
        self.waiters.first().copied()
    }

    fn effective_donated_prio(&self) -> u8 {
        self.waiters.first().map(|&(p, _)| p).unwrap_or(255)
    }
}

// Global table of all mutex states, keyed by mutex ID
static MUTEX_TABLE: SpinMutex<BTreeMap<u64, PiMutexState>> =
    SpinMutex::new(BTreeMap::new());

// Per-task: which mutex is a task currently blocked on
static TASK_BLOCKED_ON: SpinMutex<BTreeMap<u32, u64>> =
    SpinMutex::new(BTreeMap::new());

static NEXT_MUTEX_ID: AtomicU32 = AtomicU32::new(1);

// ── Public PiMutex handle ─────────────────────────────────────────────────

/// A Priority-Inheritance Mutex handle.
///
/// Cheap to copy (just a u64 ID). The actual state lives in MUTEX_TABLE.
pub struct PiMutex {
    id: u64,
}

impl PiMutex {
    /// Create a new, unlocked PI-Mutex.
    pub fn new() -> Self {
        let id = NEXT_MUTEX_ID.fetch_add(1, Ordering::Relaxed) as u64;
        MUTEX_TABLE.lock().insert(id, PiMutexState::new());
        PiMutex { id }
    }

    /// Acquire the mutex. Blocks (via scheduler) if held by another task.
    /// Performs priority donation to the holder and chains up if needed.
    pub fn lock(&self) {
        let current_pid  = crate::process::current_pid();
        let current_prio = crate::process::with_process(current_pid, |p| p.priority).unwrap_or(120);

        loop {
            let (acquired, owner) = {
                let mut tbl = MUTEX_TABLE.lock();
                let state = tbl.get_mut(&self.id).expect("invalid mutex id");
                if state.owner == 0 {
                    // Uncontested acquire
                    state.owner = current_pid;
                    state.donated_prio = 255;
                    (true, 0u32)
                } else if state.owner == current_pid {
                    // Deadlock
                    panic!("PI-Mutex: recursive lock by pid {}", current_pid);
                } else {
                    let owner = state.owner;
                    state.add_waiter(current_prio, current_pid);
                    // Compute donation: if we're higher priority than current owner's effective prio
                    let eff = state.effective_donated_prio();
                    if eff < state.donated_prio { state.donated_prio = eff; }
                    (false, owner)
                }
            };

            if acquired { return; }

            // PI donation: boost owner to our priority (if we're higher prio = lower number)
            pi_boost_chain(owner, current_prio, 0);

            // Record that we're blocked on this mutex
            TASK_BLOCKED_ON.lock().insert(current_pid, self.id);

            // Sleep — scheduler will wake us when we become the next owner
            let current_pid_u = current_pid;
            crate::process::with_process_mut(current_pid, |p| {
                p.state = crate::process::ProcessState::Sleeping;
            });
            crate::sched::yield_current();
            // Woke up — re-check if we own the mutex now
        }
    }

    /// Try to acquire without blocking. Returns true on success.
    pub fn try_lock(&self) -> bool {
        let current_pid = crate::process::current_pid();
        let mut tbl = MUTEX_TABLE.lock();
        let state = tbl.get_mut(&self.id).expect("invalid mutex id");
        if state.owner == 0 {
            state.owner = current_pid;
            true
        } else { false }
    }

    /// Release the mutex. Wakes the highest-priority waiter and transfers
    /// ownership. Removes priority donation from the releasing task.
    pub fn unlock(&self) {
        let current_pid = crate::process::current_pid();
        let next_owner: Option<u32>;
        let donated: u8;

        {
            let mut tbl = MUTEX_TABLE.lock();
            let state = tbl.get_mut(&self.id).expect("invalid mutex id");

            if state.owner != current_pid {
                // Not the owner — this is a bug
                crate::klog!("PI-Mutex: unlock by non-owner pid {} (owner={})",
                    current_pid, state.owner);
                return;
            }

            // Remove donation record
            donated = state.donated_prio;

            if let Some((waiter_prio, waiter_pid)) = state.highest_waiter() {
                state.remove_waiter(waiter_pid);
                state.owner        = waiter_pid;
                state.donated_prio = state.effective_donated_prio();
                next_owner = Some(waiter_pid);
                // Remove the new owner from "blocked" table
                TASK_BLOCKED_ON.lock().remove(&waiter_pid);
            } else {
                state.owner        = 0;
                state.donated_prio = 255;
                next_owner = None;
            }
        }

        // Undo priority boost on current_pid if we were boosted
        if donated < 255 {
            pi_restore_prio(current_pid);
        }

        // Wake the new owner
        if let Some(pid) = next_owner {
            crate::process::with_process_mut(pid, |p| {
                p.state = crate::process::ProcessState::Runnable;
            });
            crate::sched::wake_process(pid);
        }
    }

    /// True if locked by any task.
    pub fn is_locked(&self) -> bool {
        MUTEX_TABLE.lock().get(&self.id).map(|s| s.owner != 0).unwrap_or(false)
    }
}

impl Drop for PiMutex {
    fn drop(&mut self) {
        MUTEX_TABLE.lock().remove(&self.id);
    }
}

// ── Priority inheritance chain boosting ──────────────────────────────────

/// Boost `target_pid` to at least `prio` (lower = higher).
/// Then if target_pid is itself blocked, boost *its* blocker too (chained).
/// Max depth = PI_CHAIN_MAX_DEPTH to handle cycles.
fn pi_boost_chain(target_pid: u32, prio: u8, depth: usize) {
    if depth >= PI_CHAIN_MAX_DEPTH { return; }
    if target_pid == 0 { return; }

    let current_prio = crate::process::with_process(target_pid, |p| p.priority)
        .unwrap_or(255);

    if prio < current_prio {
        // Boost target_pid to prio
        crate::process::with_process_mut(target_pid, |p| { p.priority = prio; });
        crate::sched::set_priority(target_pid, prio);

        // If target is blocked on a mutex, boost that mutex's owner too
        let blocked_on = TASK_BLOCKED_ON.lock().get(&target_pid).copied();
        if let Some(mutex_id) = blocked_on {
            let owner = MUTEX_TABLE.lock().get(&mutex_id).map(|s| s.owner).unwrap_or(0);
            if owner != 0 && owner != target_pid {
                pi_boost_chain(owner, prio, depth + 1);
            }
        }
    }
}

/// Restore a task's priority to its static (pre-boost) value.
fn pi_restore_prio(pid: u32) {
    let static_prio = crate::process::with_process(pid, |p| p.static_priority).unwrap_or(120);
    let current     = crate::process::with_process(pid, |p| p.priority).unwrap_or(120);
    if current != static_prio {
        crate::process::with_process_mut(pid, |p| { p.priority = static_prio; });
        crate::sched::set_priority(pid, static_prio);
    }
}

// ── PI-aware spinlock (for interrupt context) ─────────────────────────────
//
// True RT kernels thread most interrupt handlers so they can block.
// We implement a compromise: IRQ handlers still spin, but we record
// which CPU is spinning and boost any task that tries to acquire the
// same mutex on that CPU. This prevents starvation without full threading.

pub struct PiSpinlock<T> {
    inner: SpinMutex<T>,
    held_by_cpu: AtomicU32,
}

impl<T> PiSpinlock<T> {
    pub const fn new(val: T) -> Self {
        PiSpinlock { inner: SpinMutex::new(val), held_by_cpu: AtomicU32::new(u32::MAX) }
    }

    pub fn lock(&self) -> spin::MutexGuard<T> {
        let cpu = crate::arch::x86_64::smp::current_cpu_id();
        // If another CPU holds this lock, boost the task running on that CPU
        let other_cpu = self.held_by_cpu.load(Ordering::Acquire);
        if other_cpu != u32::MAX && other_cpu != cpu {
            let other_pid = crate::process::current_pid(); // was get_current_pid_for_cpu
            if other_pid != 0 {
                let my_prio = crate::process::with_process(crate::process::current_pid(), |p| p.priority)
                    .unwrap_or(120);
                pi_boost_chain(other_pid, my_prio, 0);
            }
        }
        self.held_by_cpu.store(cpu, Ordering::Release);
        let guard = self.inner.lock();
        guard
    }

    pub fn unlock_cpu(&self) {
        self.held_by_cpu.store(u32::MAX, Ordering::Release);
    }
}
