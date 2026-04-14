/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! POSIX signal subsystem.
//!
//! Supports all 31 standard signals. Signal delivery sets up a user-space
//! signal frame so the handler runs at user privilege and returns via
//! sigreturn(2) which restores the saved register state.

use alloc::vec::Vec;
use alloc::vec;
use crate::process::Pid;

// ── Signal numbers ────────────────────────────────────────────────────────

pub const SIGHUP:    u32 = 1;
pub const SIGINT:    u32 = 2;
pub const SIGQUIT:   u32 = 3;
pub const SIGILL:    u32 = 4;
pub const SIGTRAP:   u32 = 5;
pub const SIGABRT:   u32 = 6;
pub const SIGBUS:    u32 = 7;
pub const SIGFPE:    u32 = 8;
pub const SIGKILL:   u32 = 9;
pub const SIGUSR1:   u32 = 10;
pub const SIGSEGV:   u32 = 11;
pub const SIGUSR2:   u32 = 12;
pub const SIGPIPE:   u32 = 13;
pub const SIGALRM:   u32 = 14;
pub const SIGTERM:   u32 = 15;
pub const SIGSTKFLT: u32 = 16;
pub const SIGCHLD:   u32 = 17;
pub const SIGCONT:   u32 = 18;
pub const SIGSTOP:   u32 = 19;
pub const SIGTSTP:   u32 = 20;
pub const SIGTTIN:   u32 = 21;
pub const SIGTTOU:   u32 = 22;
pub const SIGURG:    u32 = 23;
pub const SIGXCPU:   u32 = 24;
pub const SIGXFSZ:   u32 = 25;
pub const SIGVTALRM: u32 = 26;
pub const SIGPROF:   u32 = 27;
pub const SIGWINCH:  u32 = 28;
pub const SIGIO:     u32 = 29;
pub const SIGPWR:    u32 = 30;
pub const SIGSYS:    u32 = 31;

pub const NSIG: usize = 32;

// ── Signal sets ────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Default, Debug)]
pub struct SignalSet(pub u64);  // 64-bit to support real-time signals in future

impl SignalSet {
    pub const fn empty() -> Self { SignalSet(0) }
    pub fn add(&mut self, sig: u32)      { if sig < 64 { self.0 |=  1u64 << sig; } }
    pub fn remove(&mut self, sig: u32)   { if sig < 64 { self.0 &= !(1u64 << sig); } }
    pub fn has(&self, sig: u32) -> bool  { sig < 64 && self.0 & (1u64 << sig) != 0 }
    pub fn is_empty(&self) -> bool       { self.0 == 0 }
    pub fn and_not(&self, mask: &Self) -> Self { SignalSet(self.0 & !mask.0) }

    /// Return lowest pending signal number, or None.
    pub fn next_pending(&self) -> Option<u32> {
        if self.0 == 0 { None } else { Some(self.0.trailing_zeros()) }
    }
}

// ── Signal action ──────────────────────────────────────────────────────────

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SigHandler {
    Default,         // SIG_DFL
    Ignore,          // SIG_IGN
    User(u64),       // function pointer in user space
}

impl Default for SigHandler {
    fn default() -> Self { SigHandler::Default }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SigAction {
    pub handler:  SigHandler,
    pub mask:     SignalSet,    // additional mask while handling
    pub flags:    u32,
    pub restorer: u64,          // sa_restorer — user-space sigreturn trampoline
}

// SA_flags bits
pub const SA_NOCLDSTOP: u32 = 1;
pub const SA_NOCLDWAIT: u32 = 2;
pub const SA_SIGINFO:   u32 = 4;
pub const SA_ONSTACK:   u32 = 0x08000000;
pub const SA_RESTART:   u32 = 0x10000000;
pub const SA_NODEFER:   u32 = 0x40000000;
pub const SA_RESETHAND: u32 = 0x80000000;

// ── Saved signal frame on the user stack ──────────────────────────────────

/// Layout placed on the user stack before calling a signal handler.
/// On x86_64 Linux this is rt_sigframe.
#[repr(C)]
pub struct SigFrame {
    pub pretcode:   u64,     // return address → sigreturn trampoline
    pub info:       SigInfo,
    pub uc:         UContext,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SigInfo {
    pub signo:  i32,
    pub errno:  i32,
    pub code:   i32,
    pub _pad:   i32,
    pub _data:  [u64; 14],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct UContext {
    pub flags:    u64,
    pub link:     u64,
    pub stack:    [u64; 3],  // stack_t
    pub mcontext: MContext,
    pub sigmask:  u64,
    pub _fpregs:  [u64; 8],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct MContext {
    pub r8:     u64, pub r9:  u64, pub r10: u64, pub r11: u64,
    pub r12:    u64, pub r13: u64, pub r14: u64, pub r15: u64,
    pub rdi:    u64, pub rsi: u64, pub rbp: u64, pub rbx: u64,
    pub rdx:    u64, pub rax: u64, pub rcx: u64, pub rsp: u64,
    pub rip:    u64,
    pub eflags: u64,
    pub cs:     u16, pub gs: u16, pub fs: u16, pub _pad: u16,
    pub err:    u64, pub trapno: u64, pub oldmask: u64, pub cr2: u64,
    pub fpstate: u64,
    pub _reserved: [u64; 8],
}

// ── Subsystem init ────────────────────────────────────────────────────────

pub fn init() {
    crate::klog!("Signal subsystem: {} signals, user-space delivery ready", NSIG - 1);
}

// ── Signal sending ────────────────────────────────────────────────────────

/// Send `sig` to `pid`. If the signal is not masked, sets it pending.
/// SIGKILL and SIGSTOP cannot be blocked or ignored.
pub fn send_signal(pid: Pid, sig: u32) {
    if sig == 0 || sig as usize >= NSIG { return; }

    // QSF namespace: verify sender can signal this PID
    let sender = crate::process::current_pid();
    if sender != 0 && !crate::security::namespace::check_kill_permission(sender, pid, sig as i32) {
        return; // silent drop — different PID namespace
    }

    crate::process::with_process_mut(pid, |p| {
        // SIGKILL/SIGSTOP bypass mask
        let blocked = sig != SIGKILL && sig != SIGSTOP && p.sig_mask.has(sig);
        if !blocked {
            p.sig_pending.add(sig);
        }

        // Immediate state changes for unblockable signals
        match sig {
            SIGKILL => { p.state = crate::process::ProcessState::Zombie(-1); }
            SIGSTOP => { p.state = crate::process::ProcessState::Stopped; }
            _       => {}
        }
    });
}

/// Send signal to an entire process group.
pub fn send_signal_group(pgid: u32, sig: u32) {
    for pid in crate::process::all_pids() {
        if crate::process::with_process(pid, |p| p.pgid == pgid).unwrap_or(false) {
            send_signal(pid, sig);
        }
    }
}

// ── Signal dispatch ───────────────────────────────────────────────────────

/// Called from the scheduler tick for the current process.
/// Deliver pending signals to `pid`.
///
/// `from_irq`: if true, we are in interrupt/timer context.
///   - In IRQ context: ONLY apply default-disposition signals (terminate, stop,
///     ignore). User-handler signals are left pending for delivery at the next
///     syscall return.
///   - At syscall exit: deliver all pending signals including user handlers.
///
/// Why: from IRQ context `p.context.rip/rsp` are kernel callee-saved registers,
/// NOT the user-space return address. Modifying them has no effect on where the
/// process returns to user space.  User handlers must be set up at the syscall
/// boundary where the SyscallFrame on the kernel stack holds the actual user rip.
pub fn dispatch_pending(pid: Pid) {
    dispatch_pending_inner(pid, false)
}

pub fn dispatch_pending_from_irq(pid: Pid) {
    dispatch_pending_inner(pid, true)
}

fn dispatch_pending_inner(pid: Pid, from_irq: bool) {
    // irq_skip accumulates user-handler signals we see in this IRQ pass so
    // the loop can continue past them to reach default-disposition signals
    // (SIGKILL, SIGTERM, etc.) without revisiting the same user-handler sig.
    let mut irq_skip = SignalSet::empty();

    loop {
        let (pending, mask) = crate::process::with_process(pid, |p| {
            (p.sig_pending, p.sig_mask)
        }).unwrap_or((SignalSet::empty(), SignalSet::empty()));

        // In IRQ context, also exclude signals already seen-and-skipped
        let deliverable = if from_irq {
            pending.and_not(&mask).and_not(&irq_skip)
        } else {
            pending.and_not(&mask)
        };

        let sig = match deliverable.next_pending() { Some(s) => s, None => break };

        // In IRQ context, skip user-handler signals — leave them pending.
        // Default-disposition signals are safe to deliver from IRQ context
        // because apply_default() only changes process state, not CPU registers.
        if from_irq {
            let action = crate::process::with_process(pid, |p| {
                if (sig as usize) < NSIG { p.sig_actions[sig as usize] }
                else { SigAction::default() }
            }).unwrap_or_default();
            if let SigHandler::User(_) = action.handler {
                // Add to skip mask and continue — do NOT break, so we keep
                // checking lower-numbered signals that may have default disposition
                irq_skip.add(sig);
                continue;
            }
        }

        crate::process::with_process_mut(pid, |p| p.sig_pending.remove(sig));
        deliver(pid, sig);
    }
}

/// Deliver a signal using default disposition or ignore.
/// Safe to call from IRQ context (no CPU register modification).
/// User-handler signals must NOT be passed here; use deliver_with_frame().
fn deliver(pid: Pid, sig: u32) {
    let action = crate::process::with_process(pid, |p| {
        if (sig as usize) < NSIG { p.sig_actions[sig as usize] }
        else { SigAction::default() }
    }).unwrap_or_default();

    match action.handler {
        SigHandler::Ignore => {}
        // User handlers must not reach here from IRQ context.
        // dispatch_pending_from_irq() skips them via irq_skip mask.
        SigHandler::User(_) => {}
        SigHandler::Default => apply_default(pid, sig),
    }
}

/// Deliver a signal at syscall exit where we have the full SyscallFrame.
/// This is the ONLY correct path for User-handler delivery.
///
/// Modifies `frame.rip_saved` to point to the handler and calls
/// `smp::set_user_rsp(new_rsp)` so sysretq lands on the signal frame.
fn deliver_with_frame(
    pid:    Pid,
    sig:    u32,
    frame:  &mut crate::arch::x86_64::syscall_entry::SyscallFrame,
    user_rsp: u64,
) {
    let action = crate::process::with_process(pid, |p| {
        if (sig as usize) < NSIG { p.sig_actions[sig as usize] }
        else { SigAction::default() }
    }).unwrap_or_default();

    match action.handler {
        SigHandler::Ignore => {}

        SigHandler::User(handler_addr) => {
            // Build signal frame on the user stack using actual user registers
            match build_signal_frame(pid, sig, handler_addr, &action, frame, user_rsp) {
                Some(new_rsp) => {
                    // Redirect user execution to the signal handler.
                    // syscall_exit will pop frame.rip_saved into rcx → sysretq uses it as RIP.
                    frame.rip_saved = handler_addr;
                    // gs:[16] holds the user RSP that sysretq restores.
                    crate::arch::x86_64::smp::set_user_rsp(new_rsp);

                    // Update signal mask: block the delivered signal while handling it
                    crate::process::with_process_mut(pid, |p| {
                        if action.flags & SA_NODEFER == 0 {
                            p.sig_mask.add(sig);
                        }
                        for s in 0..64u32 { if action.mask.has(s) { p.sig_mask.add(s); } }
                        // SA_RESETHAND: reset to SIG_DFL after delivery
                        if action.flags & SA_RESETHAND != 0 {
                            p.sig_actions[sig as usize].handler = SigHandler::Default;
                        }
                    });
                }
                None => {
                    // Could not build frame (bad user stack) — terminate
                    apply_default(pid, sig);
                }
            }
        }

        SigHandler::Default => apply_default(pid, sig),
    }
}

/// Push a SigFrame onto the user stack and return the new RSP.
/// Build a signal frame on the user stack.
///
/// `user_rsp` is the current user-space stack pointer (from gs:[16]).
/// `user_rip` is the user-space return address (from frame.rip_saved).
/// All other user registers come from the SyscallFrame.
///
/// Returns the new user RSP (pointing at the SigFrame) on success.
fn build_signal_frame(
    pid:      Pid,
    sig:      u32,
    handler:  u64,
    action:   &SigAction,
    frame:    &crate::arch::x86_64::syscall_entry::SyscallFrame,
    user_rsp: u64,
) -> Option<u64> {
    use crate::arch::x86_64::paging::{PageMapper, phys_to_virt};

    // Collect user-register state from the SyscallFrame (the actual user
    // values at the time of the syscall, not the kernel context).
    let user_rip    = frame.rip_saved;
    let user_rflags = frame.rflags_saved;
    let (rbx, rbp, r12, r13, r14, r15) =
        (frame.rbx, frame.rbp, frame.r12, frame.r13, frame.r14, frame.r15);
    let (rax, rdi, rsi, rdx, r10, r8, r9) =
        (frame.rax, frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9);

    // Validate user RSP
    if user_rsp == 0 || user_rsp >= 0x0000_8000_0000_0000 { return None; }

    // Read current signal mask and pml4
    let (mask, pml4) = crate::process::with_process(pid, |p| {
        (p.sig_mask, p.address_space.pml4_phys)
    })?;

    let mut mapper = PageMapper::new(pml4);

    // Reserve space for SigFrame on the user stack, 16-byte aligned.
    // SysV ABI: RSP must be (frame_top % 16 == 0) at handler entry.
    let frame_size = core::mem::size_of::<SigFrame>() as u64;
    let new_rsp = (user_rsp - frame_size) & !0xFu64;

    // Sanity: new_rsp must still be in user space
    if new_rsp >= 0x0000_8000_0000_0000 { return None; }

    // Write the SigFrame through physical mappings (page-boundary safe)
    let sig_frame = SigFrame {
        pretcode: action.restorer,
        info: SigInfo { signo: sig as i32, ..Default::default() },
        uc: UContext {
            mcontext: MContext {
                // User registers — exactly what sigreturn must restore
                rsp:    user_rsp,
                rip:    user_rip,
                rax, rbx, rcx: frame.rcx, rdx,
                rsi, rdi, rbp,
                r8,  r9,  r10,
                r11:    frame.r11,
                r12, r13, r14, r15,
                eflags: user_rflags,
                oldmask: mask.0,
                ..Default::default()
            },
            sigmask: mask.0,
            ..Default::default()
        },
    };

    let bytes = unsafe {
        core::slice::from_raw_parts(
            &sig_frame as *const SigFrame as *const u8,
            core::mem::size_of::<SigFrame>(),
        ).to_vec()
    };
    write_to_user_va(&mut mapper, new_rsp, &bytes)?;

    Some(new_rsp)
}

/// Write `data` bytes to virtual address `va` in the address space given by
/// `mapper`, correctly handling page boundaries.
fn write_to_user_va(
    mapper: &mut crate::arch::x86_64::paging::PageMapper,
    mut va: u64,
    data: &[u8],
) -> Option<()> {
    use crate::arch::x86_64::paging::phys_to_virt;
    let mut remaining = data;
    while !remaining.is_empty() {
        let page_off = (va & 0xFFF) as usize;
        let phys = unsafe { mapper.translate(va) }?;
        let page_base_virt = phys_to_virt(phys & !0xFFF);
        let can_write = (0x1000 - page_off).min(remaining.len());
        unsafe {
            core::ptr::copy_nonoverlapping(
                remaining.as_ptr(),
                (page_base_virt + page_off as u64) as *mut u8,
                can_write,
            );
        }
        va        += can_write as u64;
        remaining  = &remaining[can_write..];
    }
    Some(())
}

fn apply_default(pid: Pid, sig: u32) {
    use crate::process::ProcessState;

    // SIGCONT appears twice below intentionally: it matches "cont" in the
    // first arm. The second _ arm catches everything else.
    let action = match sig {
        SIGCHLD | SIGWINCH | SIGURG => "ignore",
        SIGSTOP | SIGTSTP | SIGTTIN | SIGTTOU => "stop",
        SIGCONT => "cont",
        _ => "terminate",
    };

    match action {
        "ignore" => {}

        "stop" => {
            crate::process::with_process_mut(pid, |p| p.state = ProcessState::Stopped);
            crate::sched::remove_task(pid);
        }

        "cont" => {
            // A stopped process was removed from the run queue by remove_task().
            // wake_process() only handles Sleeping→Runnable (process still has a
            // SchedEntity). For Stopped→Runnable we must use add_task() to create
            // a fresh SchedEntity.
            let was_stopped = crate::process::with_process(pid, |p| {
                p.state == ProcessState::Stopped
            }).unwrap_or(false);

            crate::process::with_process_mut(pid, |p| {
                if p.state == ProcessState::Stopped {
                    p.state = ProcessState::Runnable;
                }
            });

            if was_stopped {
                // Re-add to scheduler with normal priority
                crate::sched::add_task(pid, crate::sched::PRIO_NORMAL, crate::sched::SCHED_NORMAL);
            } else {
                // May have been sleeping (e.g. SIGSTOP followed immediately by SIGCONT)
                crate::sched::wake_process(pid);
            }
        }

        _ => {
            // Terminating signal: mark zombie, notify parent, remove from scheduler.
            let exit_code = -(sig as i32);
            let ppid;
            {
                let t_ref = crate::process::with_process_mut(pid, |p| {
                    p.state     = ProcessState::Zombie(exit_code);
                    p.exit_code = exit_code;
                    p.ppid
                });
                ppid = t_ref.unwrap_or(0);
            }
            // Remove from run queue so it doesn't run as a zombie
            crate::sched::remove_task(pid);
            // Notify parent
            if ppid != 0 {
                send_signal(ppid, SIGCHLD);
                crate::sched::wake_process(ppid);
            }
            // If this is the current process, yield to scheduler
            if pid == crate::process::current_pid() {
                crate::sched::schedule_next_from_irq();
            }
        }
    }
}

/// Called at the end of every syscall dispatch, before returning to user space.
/// Delivers ALL pending deliverable signals.
///
/// User-handler signals are delivered by patching the SyscallFrame so that
/// sysretq jumps to the handler instead of the original user return address.
/// Default-disposition signals (terminate/stop/ignore) change process state.
///
/// Only one user handler is delivered per syscall boundary (the next syscall
/// or signal frame's sigreturn will deliver the next one, as on Linux).
pub fn deliver_pending_at_syscall_exit(
    frame: &mut crate::arch::x86_64::syscall_entry::SyscallFrame,
) {
    let pid      = crate::process::current_pid();
    let user_rsp = crate::arch::x86_64::smp::get_user_rsp();

    loop {
        let (pending, mask) = crate::process::with_process(pid, |p| {
            (p.sig_pending, p.sig_mask)
        }).unwrap_or((SignalSet::empty(), SignalSet::empty()));

        let deliverable = pending.and_not(&mask);
        let sig = match deliverable.next_pending() { Some(s) => s, None => break };

        // Remove from pending before delivery (prevents re-entrancy)
        crate::process::with_process_mut(pid, |p| p.sig_pending.remove(sig));

        let action = crate::process::with_process(pid, |p| {
            if (sig as usize) < NSIG { p.sig_actions[sig as usize] }
            else { SigAction::default() }
        }).unwrap_or_default();

        match action.handler {
            SigHandler::Ignore => {
                // Signal is consumed, continue to check next
            }
            SigHandler::User(_) => {
                // Deliver user handler — this patches the frame to jump to
                // the handler; subsequent signals will be delivered on return
                // from the handler (via sigreturn → next syscall boundary).
                deliver_with_frame(pid, sig, frame, user_rsp);
                // After redirecting to a user handler, stop processing signals.
                // The handler will run, call sigreturn, and pending signals
                // will be checked again on the next syscall exit.
                break;
            }
            SigHandler::Default => {
                apply_default(pid, sig);
                // If we just terminated or stopped ourselves, stop processing
                let dead = crate::process::with_process(pid, |p| {
                    p.is_zombie() || p.state == crate::process::ProcessState::Stopped
                }).unwrap_or(true);
                if dead { break; }
            }
        }
    }
}

// ── sigreturn — restores context saved in signal frame ────────────────────

/// Called from sys_rt_sigreturn. Restores CPU context from the signal frame
/// that was placed on the user stack during delivery.
/// Restore user context after a signal handler returns.
///
/// ## How signal delivery and sigreturn pair up
///
/// **Delivery** (`deliver_with_frame`):
///   1. Saved the complete user register set into a `SigFrame` on the user stack.
///   2. Patched `frame.rip_saved = handler_addr`  → sysretq jumps into handler.
///   3. Called `smp::set_user_rsp(sig_frame_rsp)` → sysretq loads handler stack.
///
/// **Sigreturn** (this function):
///   1. `gs:[16]` (via `smp::get_user_rsp()`) still points at the `SigFrame`.
///   2. We read the `SigFrame` from that user address.
///   3. We restore all registers **into the SyscallFrame** (what `syscall_exit`
///      pops) and restore user RSP via `smp::set_user_rsp`.
///
/// We MUST NOT write to `p.context.*` here — those are the *kernel* callee-saved
/// registers used by `context_switch`, not the registers visible to userspace.
pub fn sigreturn(syscall_frame: &mut crate::arch::x86_64::syscall_entry::SyscallFrame) -> i64 {
    use crate::arch::x86_64::paging::{PageMapper, phys_to_virt};

    // The signal frame is at the user RSP stored in gs:[16].
    // deliver_with_frame set gs:[16] = new_rsp (pointing at the SigFrame)
    // before redirecting execution to the handler.  The handler does not
    // modify gs:[16] (it is a kernel-only slot), so it still holds the
    // SigFrame address when sigreturn is called.
    let sig_frame_rsp = crate::arch::x86_64::smp::get_user_rsp();

    // Validate: must be a canonical user-space address
    if sig_frame_rsp == 0 || sig_frame_rsp >= 0x0000_8000_0000_0000 {
        return -14; // EFAULT
    }

    let pml4 = match crate::process::with_current(|p| p.address_space.pml4_phys) {
        Some(v) => v,
        None    => return -14,
    };

    // Read the SigFrame page-by-page to handle page-boundary crossings
    let frame_size = core::mem::size_of::<SigFrame>();
    let mut frame_bytes = alloc::vec![0u8; frame_size];
    {
        let mut mapper = PageMapper::new(pml4);
        let mut va  = sig_frame_rsp;
        let mut pos = 0usize;
        while pos < frame_size {
            let page_off = (va & 0xFFF) as usize;
            let phys = match unsafe { mapper.translate(va) } {
                Some(p) => p,
                None    => return -14, // EFAULT: unmapped page
            };
            let chunk = (0x1000 - page_off).min(frame_size - pos);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (phys_to_virt(phys & !0xFFF) + page_off as u64) as *const u8,
                    frame_bytes[pos..].as_mut_ptr(),
                    chunk,
                );
            }
            va  += chunk as u64;
            pos += chunk;
        }
    }

    let sig_frame: &SigFrame = unsafe { &*(frame_bytes.as_ptr() as *const SigFrame) };
    let mc = &sig_frame.uc.mcontext;

    // Validate the restored instruction pointer is in canonical user space
    if mc.rip == 0 || mc.rip >= 0x0000_8000_0000_0000 {
        return -14; // EFAULT: would jump into kernel space
    }

    // Restore signal mask from the saved state in the signal frame
    crate::process::with_current_mut(|p| {
        p.sig_mask = SignalSet(sig_frame.uc.sigmask);
    });

    // Restore user-space registers into the SyscallFrame.
    // syscall_exit pops these directly onto the CPU before sysretq, so
    // whatever we write here becomes the actual register values in userland.
    syscall_frame.rip_saved    = mc.rip;        // → rcx before sysretq → RIP
    syscall_frame.rflags_saved = mc.eflags      // → r11 before sysretq → RFLAGS
                                 & 0x0000_0000_0003_F7FF // safe RFLAGS mask
                                 | 0x0000_0000_0000_0200; // always IF=1
    syscall_frame.rbx  = mc.rbx;
    syscall_frame.rbp  = mc.rbp;
    syscall_frame.r12  = mc.r12;
    syscall_frame.r13  = mc.r13;
    syscall_frame.r14  = mc.r14;
    syscall_frame.r15  = mc.r15;
    syscall_frame.rdi  = mc.rdi;
    syscall_frame.rsi  = mc.rsi;
    syscall_frame.rdx  = mc.rdx;
    syscall_frame.r10  = mc.r10;
    syscall_frame.r8   = mc.r8;
    syscall_frame.r9   = mc.r9;
    syscall_frame.rcx  = mc.rcx;
    // rax is set by the return value of this function (syscall_dispatch_rs returns
    // it, then syscall_exit skips over the frame.rax slot).
    // The signal handler's return value in rax is mc.rax.

    // Restore user stack pointer via the gs:[16] slot — sysretq loads this
    // into RSP before entering user mode.
    crate::arch::x86_64::smp::set_user_rsp(mc.rsp);

    // Return mc.rax: this becomes the user rax after sysretq (the value the
    // interrupted code sees as the return value of whatever was interrupted).
    mc.rax as i64
}

// ── Compatibility aliases ─────────────────────────────────────────────────

// These allow older-style SigDispositionRepr usage to keep compiling
pub type SigDispositionRepr = SigHandler;
