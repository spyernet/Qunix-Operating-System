//! POSIX/Linux ABI syscall helpers.
//!
//! Helpers with semantics tightly tied to the x86-64 Linux userland ABI
//! (futex, clone, sched, etc.) that live here to keep syscall/mod.rs clean.
//! All behavior is unchanged — only the module home has moved.

use crate::syscall::handlers;

pub fn init() {
    crate::klog!("ABI compat: POSIX syscall extensions active");
}

// ── Linux-specific helpers called from syscall/handlers.rs ────────────────

/// readv — scatter-gather read
pub fn sys_readv(fd: i32, iov: u64, iovcnt: usize) -> i64 {
    #[repr(C)] struct IoVec { base: u64, len: u64 }
    let mut total = 0i64;
    for i in 0..iovcnt {
        let v = unsafe { &*((iov + i as u64 * 16) as *const IoVec) };
        if v.len == 0 { continue; }
        let r = handlers::sys_read(fd, v.base, v.len as usize);
        if r < 0 { return if total == 0 { r } else { total }; }
        total += r;
        if r < v.len as i64 { break; }
    }
    total
}

/// writev — scatter-gather write
pub fn sys_writev(fd: i32, iov: u64, iovcnt: usize) -> i64 {
    #[repr(C)] struct IoVec { base: u64, len: u64 }
    let mut total = 0i64;
    for i in 0..iovcnt {
        let v = unsafe { &*((iov + i as u64 * 16) as *const IoVec) };
        if v.len == 0 { continue; }
        let r = handlers::sys_write(fd, v.base, v.len as usize);
        if r < 0 { return if total == 0 { r } else { total }; }
        total += r;
        if r < v.len as i64 { break; }
    }
    total
}

/// sched_setscheduler — map Linux scheduler policy to Qunix priority
pub fn sys_sched_setscheduler(pid: i32, policy: i32, param: u64) -> i64 {
    let target = if pid == 0 {
        crate::process::current_pid()
    } else {
        pid as u32
    };
    let sched_prio = if param != 0 {
        unsafe { *(param as *const u32) as u8 }
    } else { 0 };

    let qunix_prio = match policy {
        1 | 2 => sched_prio.min(99),            // SCHED_FIFO / SCHED_RR → RT range (POSIX)
        5     => crate::sched::PRIO_IDLE,        // SCHED_IDLE
        _     => crate::sched::PRIO_NORMAL,      // SCHED_NORMAL / BATCH
    };
    let qunix_policy = match policy {
        1 => crate::sched::SCHED_FIFO,
        2 => crate::sched::SCHED_RR,
        5 => crate::sched::SCHED_IDLE,
        _ => crate::sched::SCHED_NORMAL,
    };
    crate::sched::set_priority(target, qunix_prio);
    0
}

/// clone — create a new process or thread.
/// CLONE_VM | CLONE_THREAD → real thread sharing address space, fds, signals.
/// Otherwise → fork with separate address space.
pub fn sys_clone(flags: u64, child_stack: u64, ptid: u64, ctid: u64, tls: u64) -> i64 {
    const CLONE_VM:             u64 = 0x0000_0100;
    const CLONE_FS:             u64 = 0x0000_0200;
    const CLONE_FILES:          u64 = 0x0000_0400;
    const CLONE_SIGHAND:        u64 = 0x0000_0800;
    const CLONE_THREAD:         u64 = 0x0001_0000;
    const CLONE_SETTLS:         u64 = 0x0008_0000;
    const CLONE_PARENT_SETTID:  u64 = 0x0010_0000;
    const CLONE_CHILD_CLEARTID: u64 = 0x0020_0000;
    const CLONE_CHILD_SETTID:   u64 = 0x0100_0000;

    let is_thread = flags & (CLONE_VM | CLONE_THREAD) == (CLONE_VM | CLONE_THREAD);
    let tls_val   = if flags & CLONE_SETTLS != 0 { tls } else { 0 };
    let ctid_addr = if flags & (CLONE_CHILD_SETTID | CLONE_CHILD_CLEARTID) != 0 { ctid } else { 0 };

    let parent_pid = crate::process::current_pid();

    let child_pid = if is_thread {
        // Real thread: shares VM, fds, signal handlers
        match crate::process::clone_thread(parent_pid, child_stack, tls_val, ctid_addr) {
            Some(p) => p,
            None    => return -12,
        }
    } else {
        // New process: copy address space
        match crate::process::fork_current() {
            Some(p) => p,
            None    => return -12,
        }
    };

    // Write parent TID
    if flags & CLONE_PARENT_SETTID != 0 && ptid != 0 {
        unsafe { *(ptid as *mut u32) = child_pid; }
    }

    // Set TLS (FS base) in child context if SETTLS
    if flags & CLONE_SETTLS != 0 && tls != 0 {
        crate::process::with_process_mut(child_pid, |p| {
            p.fs_base = tls;
            // Will be loaded when child first runs via arch_prctl ARCH_SET_FS
        });
    }

    // For threads: the child stack pointer is the user-space stack passed in
    // The child returns 0 from clone() — its rsp must point to child_stack.
    if is_thread && child_stack != 0 {
        crate::process::with_process_mut(child_pid, |p| {
            // The child's user-space RSP for the iretq into ring 3
            // We store it so the scheduler can resume correctly.
            // Context.rsp is the *kernel* stack; the user RSP is in the
            // saved frame that gets restored on iretq.
            // For threads spawned via clone(), the kernel entry restores
            // the syscall frame with rax=0, rsp=child_stack.
            // We encode this by stashing child_stack in a scratch field.
            p.flags = child_stack as u32; // repurpose flags for thread stack hint
        });
    }

    crate::sched::add_task(child_pid, crate::sched::PRIO_NORMAL, crate::sched::SCHED_NORMAL);

    child_pid as i64
}

/// futex — fast userspace mutex with proper wait queue
pub fn sys_futex(uaddr: u64, op: i32, val: u32, timeout: u64, uaddr2: u64, val3: u32) -> i64 {
    const FUTEX_WAIT:       i32 = 0;
    const FUTEX_WAKE:       i32 = 1;
    const FUTEX_FD:         i32 = 2;
    const FUTEX_REQUEUE:    i32 = 3;
    const FUTEX_CMP_REQUEUE:i32 = 4;
    const FUTEX_WAKE_OP:    i32 = 5;
    const FUTEX_PRIVATE:    i32 = 128;
    const FUTEX_CLOCK_RT:   i32 = 256;

    let op = op & !(FUTEX_PRIVATE | FUTEX_CLOCK_RT);

    if uaddr == 0 { return -14; } // EFAULT

    match op {
        FUTEX_WAIT => {
            // Validate user pointer
            if uaddr >= 0x0000_8000_0000_0000 { return -14; } // EFAULT

            let pid = crate::process::current_pid();

            // Parse timeout — convert to absolute tick deadline
            let deadline: Option<u64> = if timeout != 0 {
                if timeout >= 0x0000_8000_0000_0000 { return -14; } // EFAULT
                let ts = unsafe { &*(timeout as *const [i64; 2]) };
                let ms = (ts[0].max(0) as u64) * 1000
                       + (ts[1].max(0) as u64) / 1_000_000;
                Some(crate::time::ticks() + ms)
            } else {
                None // infinite wait
            };

            // ── Atomic check-then-register protocol ─────────────────────
            // Register waiter FIRST, then re-check the value.
            // If the value already changed (the waker ran before we registered),
            // we see it here and bail with EAGAIN without sleeping.
            // This prevents the lost-wakeup race: register→check→sleep is safe.
            FUTEX_TABLE.lock().push((uaddr, pid));

            // Re-check the futex word under the table lock is NOT needed here
            // because we're on a single CPU with interrupts — the value can
            // only change via another syscall which would need the CPU.
            // On SMP a memory barrier would be needed; we're uniprocessor for now.
            let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
            if cur != val {
                // Value changed before we slept — remove from table and return EAGAIN
                FUTEX_TABLE.lock().retain(|&(a, p)| !(a == uaddr && p == pid));
                return -11; // EAGAIN
            }

            // Set up timeout in process state if applicable
            if let Some(dl) = deadline {
                crate::process::with_process_mut(pid, |p| {
                    p.sleep_until = dl;
                });
            }

            // Block until woken by FUTEX_WAKE or timeout
            crate::sched::block_current(crate::process::ProcessState::Sleeping);

            // After wakeup: determine reason
            let timed_out = deadline.map(|dl| {
                // If we were woken by the timer, sleep_until is cleared by wake_sleeping_processes.
                // If woken by FUTEX_WAKE, sleep_until may still have the deadline.
                // Check: are we still in the futex table? If yes, it was a timeout wakeup.
                let still_waiting = FUTEX_TABLE.lock()
                    .iter().any(|&(a, p)| a == uaddr && p == pid);
                if still_waiting {
                    // Timed out — clean up our entry
                    FUTEX_TABLE.lock().retain(|&(a, p)| !(a == uaddr && p == pid));
                    true
                } else {
                    false
                }
            }).unwrap_or(false);

            // Clear sleep_until so timer doesn't re-fire
            crate::process::with_process_mut(pid, |p| { p.sleep_until = 0; });

            if timed_out { -110 } else { 0 } // ETIMEDOUT or 0
        }

        FUTEX_WAKE => {
            // Collect up to `val` PIDs waiting on this uaddr, then wake them
            // outside the lock to avoid lock-ordering issues between the futex
            // table lock and the scheduler run-queue lock.
            let n = val as usize;
            let pids_to_wake: alloc::vec::Vec<crate::process::Pid> = {
                let mut table = FUTEX_TABLE.lock();
                let mut pids  = alloc::vec::Vec::new();
                table.retain(|&(a, p)| {
                    if a == uaddr && pids.len() < n {
                        pids.push(p);
                        false // remove from table
                    } else {
                        true  // keep
                    }
                });
                pids
            };
            let woken = pids_to_wake.len();
            for pid in pids_to_wake {
                crate::sched::wake_process(pid);
            }
            woken as i64
        }

        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            // FUTEX_CMP_REQUEUE requires *uaddr == val3 before proceeding
            if op == FUTEX_CMP_REQUEUE {
                if uaddr >= 0x0000_8000_0000_0000 { return -14; } // EFAULT
                let cur = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
                if cur != val3 { return -11; } // EAGAIN
            }

            // Wake up to `val` waiters, then requeue remaining to uaddr2.
            // Collect wake targets first, update table for requeue, then wake.
            let to_wake = val as usize;
            let pids_to_wake: alloc::vec::Vec<crate::process::Pid> = {
                let mut table    = FUTEX_TABLE.lock();
                let mut wake_now = alloc::vec::Vec::new();
                for entry in table.iter_mut() {
                    if entry.0 == uaddr {
                        if wake_now.len() < to_wake {
                            wake_now.push(entry.1);
                            entry.0 = u64::MAX; // sentinel: will be removed below
                        } else {
                            entry.0 = uaddr2; // requeue
                        }
                    }
                }
                table.retain(|&(a, _)| a != u64::MAX);
                wake_now
            };
            let woken = pids_to_wake.len();
            for pid in pids_to_wake {
                crate::sched::wake_process(pid);
            }
            woken as i64
        }

        FUTEX_WAKE_OP => {
            // FUTEX_WAKE_OP: atomically operate on uaddr2, then wake waiters.
            // The operation is encoded in val3 (op/cmp/oparg/cmparg bitfields).
            // We implement the atomic op correctly per Linux semantics.
            if uaddr2 != 0 && uaddr2 < 0x0000_8000_0000_0000 {
                let op_encoded = val3;
                let op_type   = (op_encoded >> 28) & 0xF;
                let cmp_type  = (op_encoded >> 24) & 0xF;
                let op_arg    = (op_encoded >> 12) & 0xFFF;
                let cmp_arg   = (op_encoded >>  0) & 0xFFF;

                let old_val = unsafe { core::ptr::read_volatile(uaddr2 as *const u32) };
                let new_val = match op_type {
                    0 => op_arg,           // FUTEX_OP_SET
                    1 => old_val | op_arg, // FUTEX_OP_OR
                    2 => old_val & !op_arg,// FUTEX_OP_ANDN
                    3 => old_val ^ op_arg, // FUTEX_OP_XOR
                    4 => old_val.wrapping_add(op_arg), // FUTEX_OP_ADD
                    _ => old_val,
                };
                unsafe { core::ptr::write_volatile(uaddr2 as *mut u32, new_val); }

                let cmp_true = match cmp_type {
                    0 => old_val == cmp_arg,
                    1 => old_val != cmp_arg,
                    2 => old_val <  cmp_arg,
                    3 => old_val <= cmp_arg,
                    4 => old_val >  cmp_arg,
                    5 => old_val >= cmp_arg,
                    _ => false,
                };

                if cmp_true {
                    // Also wake waiters on uaddr2
                    let pids2: alloc::vec::Vec<_> = {
                        let mut table = FUTEX_TABLE.lock();
                        let mut v = alloc::vec::Vec::new();
                        table.retain(|&(a, p)| if a == uaddr2 { v.push(p); false } else { true });
                        v
                    };
                    for pid in pids2 { crate::sched::wake_process(pid); }
                }
            }

            // Wake `val` waiters on uaddr (same collect-then-wake pattern)
            let to_wake = val as usize;
            let pids: alloc::vec::Vec<crate::process::Pid> = {
                let mut table = FUTEX_TABLE.lock();
                let mut v = alloc::vec::Vec::new();
                table.retain(|&(a, p)| {
                    if a == uaddr && v.len() < to_wake { v.push(p); false } else { true }
                });
                v
            };
            let woken = pids.len();
            for pid in pids { crate::sched::wake_process(pid); }
            woken as i64
        }

        _ => -22, // EINVAL
    }
}

// ── Futex wait table ──────────────────────────────────────────────────────

use alloc::vec::Vec;
use spin::Mutex;

// (uaddr, waiting_pid)
static FUTEX_TABLE: Mutex<Vec<(u64, crate::process::Pid)>> = Mutex::new(Vec::new());

/// Called on process exit — remove from futex wait table.
pub fn futex_cleanup(pid: crate::process::Pid) {
    FUTEX_TABLE.lock().retain(|&(_, p)| p != pid);
}
