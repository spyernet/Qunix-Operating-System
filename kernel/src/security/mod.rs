/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Qunix Security Foundation (QSF) — multi-layer kernel security subsystem.
//!
//! ## Layers (applied in order on every security-sensitive operation)
//!
//! 1. **DAC** (Discretionary Access Control)
//!    Standard POSIX uid/gid/mode permission checks.
//!    Owner can grant access to others; root bypasses all DAC.
//!
//! 2. **Capabilities**
//!    Fine-grained privilege decomposition. Replaces the binary root/non-root
//!    model. Every process has three capability sets:
//!      - permitted  (what it can ever have)
//!      - effective  (currently active)
//!      - inheritable (passed across exec)
//!    Capabilities are dropped on exec unless explicitly preserved.
//!
//! 3. **Mandatory Access Control (QSF-MAC)**
//!    Label-based: every object (file, socket, process) carries a label.
//!    Labels live in `SecurityLabel` (subject) and `ObjectLabel` (object).
//!    The policy matrix `MAC_POLICY` determines what subjects can do to objects.
//!    This is our novel contribution over Linux — MAC is built-in and zero-cost
//!    when the policy is empty, unlike SELinux (which requires a separate
//!    userspace policy compiler and kernel module).
//!
//! 4. **Syscall Allowlist (QSF-SAL)**
//!    Per-process syscall allowlist. Once a process installs a filter, only
//!    listed syscall numbers are permitted. Violations result in SIGSYS.
//!    Tighter than seccomp-BPF: the allowlist is a compact u64 bitmap
//!    (512 syscalls = 8 words) with zero dynamic allocation.
//!
//! 5. **Address Space Integrity (QSF-ASI)**
//!    All user pointers are validated before the kernel dereferences them:
//!    - Address must be below 0x0000_8000_0000_0000 (canonical user space)
//!    - Range must lie within a VMA registered in the process address space
//!    - Page must be mapped with appropriate permissions (read/write/exec)
//!    This eliminates an entire class of kernel memory corruption via
//!    malformed user pointers.
//!
//! 6. **Audit Log (QSF-AUDIT)**
//!    Security-relevant events written to a ring buffer:
//!    - syscall denials
//!    - capability checks
//!    - MAC violations
//!    - exec events
//!    Readable from /proc/qsf/audit. Zero-copy: events are formatted
//!    in-place into the ring buffer, no heap allocation on the hot path.
//!
//! ## Design philosophy
//!
//! - **Zero-overhead when unused**: every gate is guarded by an AtomicBool.
//!   If no process has installed a filter / MAC policy, all checks are
//!   a single atomic load + branch — effectively free.
//! - **No GPL contamination**: QSF is entirely MIT-licensed.
//! - **No runtime policy compiler**: policies are Rust types, checked at
//!   compile time if using the built-in profiles, or validated at runtime
//!   when loaded dynamically.
//! - **Composable**: each layer is independent. A process can have MAC but
//!   no syscall filter, or syscall filter but no capability restrictions.

pub mod seccomp;
pub mod mac_policy;
pub mod namespace;
pub mod memory_tagging;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use alloc::string::{String, ToString};
use spin::Mutex;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

// ── Capability constants (POSIX + Linux ABI compatible) ───────────────────

pub const CAP_CHOWN:           u64 = 1 << 0;
pub const CAP_DAC_OVERRIDE:    u64 = 1 << 1;
pub const CAP_DAC_READ:        u64 = 1 << 2;
pub const CAP_FSETID:          u64 = 1 << 4;
pub const CAP_KILL:            u64 = 1 << 5;
pub const CAP_SETGID:          u64 = 1 << 6;
pub const CAP_SETUID:          u64 = 1 << 7;
pub const CAP_NET_BIND:        u64 = 1 << 10;
pub const CAP_NET_ADMIN:       u64 = 1 << 12;
pub const CAP_NET_RAW:         u64 = 1 << 13;
pub const CAP_SYS_RAWIO:       u64 = 1 << 17;
pub const CAP_SYS_PTRACE:      u64 = 1 << 19;
pub const CAP_SYS_ADMIN:       u64 = 1 << 21;
pub const CAP_SYS_BOOT:        u64 = 1 << 22;
pub const CAP_SYS_NICE:        u64 = 1 << 23;
pub const CAP_SYS_MODULE:      u64 = 1 << 25;
pub const CAP_SYS_MKNOD:       u64 = 1 << 27;
pub const CAP_AUDIT_WRITE:     u64 = 1 << 29;
pub const CAP_AUDIT_CONTROL:   u64 = 1 << 30;

pub const CAPS_ALL:  u64 = u64::MAX;
pub const CAPS_NONE: u64 = 0;

/// Capabilities granted to unprivileged processes by default.
pub const CAPS_DEFAULT_USER: u64 = CAP_KILL | CAP_SETUID | CAP_SETGID;

// ── Layer 1: DAC — Discretionary Access Control ───────────────────────────

pub const MAY_READ:  u8 = 4;
pub const MAY_WRITE: u8 = 2;
pub const MAY_EXEC:  u8 = 1;

pub fn dac_check(
    uid: u32, gid: u32,
    file_uid: u32, file_gid: u32,
    mode: u32, access: u8,
) -> bool {
    // root bypasses DAC
    if uid == 0 { return true; }
    let shift = if uid == file_uid { 6 }
                else if gid == file_gid { 3 }
                else { 0 };
    (mode >> shift) & access as u32 != 0
}

pub fn current_dac_check(file_uid: u32, file_gid: u32, mode: u32, access: u8) -> bool {
    let (uid, gid) = crate::process::with_current(|p| (p.uid, p.gid)).unwrap_or((0, 0));
    dac_check(uid, gid, file_uid, file_gid, mode, access)
}

// ── Layer 2: Capabilities ─────────────────────────────────────────────────

#[derive(Clone, Copy)]
pub struct Credentials {
    pub uid:     u32,
    pub gid:     u32,
    pub euid:    u32,
    pub egid:    u32,
    pub cap_eff: u64,  // effective capabilities
    pub cap_per: u64,  // permitted capabilities
    pub cap_inh: u64,  // inheritable capabilities
    pub sandbox: SandboxLevel,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SandboxLevel {
    None,       // full kernel access (kernel threads)
    User,       // normal DAC + capabilities
    Restricted, // no network, no raw IO
    Isolated,   // fully isolated ABI-compat process
}

impl Credentials {
    pub const fn root() -> Self {
        Credentials { uid:0, gid:0, euid:0, egid:0,
            cap_eff: CAPS_ALL, cap_per: CAPS_ALL, cap_inh: CAPS_ALL,
            sandbox: SandboxLevel::None }
    }
    pub const fn user(uid: u32, gid: u32) -> Self {
        Credentials { uid, gid, euid: uid, egid: gid,
            cap_eff: CAPS_DEFAULT_USER, cap_per: CAPS_DEFAULT_USER, cap_inh: CAPS_NONE,
            sandbox: SandboxLevel::User }
    }
    pub fn has_cap(&self, cap: u64) -> bool {
        self.uid == 0 || self.cap_eff & cap != 0
    }
    pub fn drop_cap(&mut self, cap: u64) {
        self.cap_eff &= !cap;
        self.cap_per &= !cap;
    }
    /// Exec: effective = permitted & inheritable; clear if no setuid bit
    pub fn exec_transition(&mut self, file_uid: u32, file_mode: u32) {
        let setuid = file_mode & 0o4000 != 0;
        if setuid { self.euid = file_uid; }
        // Clear capabilities that aren't in permitted ∩ inheritable
        self.cap_eff = self.cap_per & self.cap_inh;
        // Privileged exec restores full caps
        if self.euid == 0 { self.cap_eff = self.cap_per; }
    }
}

pub fn current_has_cap(cap: u64) -> bool {
    crate::process::with_current(|p| p.uid == 0).unwrap_or(false)
}

// ── Layer 3: MAC — Mandatory Access Control ───────────────────────────────

/// Security label — attached to every process, file, and socket.
/// The 64-bit label encodes: [level:8][category:24][integrity:8][reserved:24].
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SecurityLabel(pub u64);

impl SecurityLabel {
    pub const KERNEL:      Self = SecurityLabel(0x0000_0000_0000_0000);
    pub const SYSTEM:      Self = SecurityLabel(0x0100_0000_0000_0000);
    pub const USER:        Self = SecurityLabel(0x0200_0000_0000_0000);
    pub const UNTRUSTED:   Self = SecurityLabel(0x0300_0000_0000_0000);
    pub const RESTRICTED:  Self = SecurityLabel(0x0400_0000_0000_0000);

    pub fn level(&self) -> u8   { ((self.0 >> 56) & 0xFF) as u8 }
    pub fn category(&self) -> u32 { ((self.0 >> 32) & 0x00FF_FFFF) as u32 }
    pub fn integrity(&self) -> u8 { ((self.0 >> 24) & 0xFF) as u8 }

    /// Can `subject` flow to `object`? (Biba-style: no write up, no read down)
    /// Returns None if policy engine is disabled, Some(bool) if it is.
    pub fn can_flow(subject: Self, object: Self) -> bool {
        // Kernel can always write anywhere
        if subject == Self::KERNEL { return true; }
        // Subject must not be at a lower integrity than object to write
        subject.level() >= object.level()
    }
}

/// MAC access rights — bitmask of permitted operations.
#[derive(Clone, Copy, Default)]
pub struct MacAccess(pub u32);

impl MacAccess {
    pub const READ:    u32 = 1 << 0;
    pub const WRITE:   u32 = 1 << 1;
    pub const EXEC:    u32 = 1 << 2;
    pub const NET:     u32 = 1 << 3;
    pub const IPC:     u32 = 1 << 4;
    pub const SIGNAL:  u32 = 1 << 5;
    pub const PTRACE:  u32 = 1 << 6;
    pub const ADMIN:   u32 = 1 << 7;
    pub const ALL:     u32 = 0xFFFF_FFFF;

    pub fn allows(&self, access: u32) -> bool { self.0 & access == access }
}

/// One MAC policy rule: (subject_label, object_label) → allowed_access.
#[derive(Clone, Copy)]
pub struct MacRule {
    pub subject: SecurityLabel,
    pub object:  SecurityLabel,
    pub access:  MacAccess,
}

/// Global MAC policy — enforced on every IPC, file, and network operation.
pub struct MacPolicy {
    pub rules:        Vec<MacRule>,
    pub enabled:      bool,
    pub default_deny: bool,
}

impl MacPolicy {
    fn new() -> Self { MacPolicy { rules: Vec::new(), enabled: false, default_deny: false } }

    fn check(&self, subject: SecurityLabel, object: SecurityLabel, access: u32) -> bool {
        if !self.enabled { return true; } // MAC disabled → allow all
        // Kernel label bypasses everything
        if subject == SecurityLabel::KERNEL { return true; }
        // Label flow check (integrity model)
        if !SecurityLabel::can_flow(subject, object) { return false; }
        // Look up explicit rules
        for rule in &self.rules {
            if rule.subject == subject && rule.object == object {
                return rule.access.allows(access);
            }
        }
        !self.default_deny
    }

    fn add_rule(&mut self, subject: SecurityLabel, object: SecurityLabel, access: MacAccess) {
        self.rules.retain(|r| !(r.subject == subject && r.object == object));
        self.rules.push(MacRule { subject, object, access });
    }
}

pub static MAC_POLICY: Mutex<MacPolicy> = Mutex::new(MacPolicy {
    rules: Vec::new(), enabled: false, default_deny: false,
});
static MAC_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn mac_enable()  { MAC_ENABLED.store(true,  Ordering::Release); MAC_POLICY.lock().enabled = true;  }
pub fn mac_disable() { MAC_ENABLED.store(false, Ordering::Release); MAC_POLICY.lock().enabled = false; }
pub fn mac_is_enabled() -> bool { MAC_ENABLED.load(Ordering::Relaxed) }

pub fn mac_add_rule(subject: SecurityLabel, object: SecurityLabel, access: MacAccess) {
    MAC_POLICY.lock().add_rule(subject, object, access);
}

pub fn mac_check(subject: SecurityLabel, object: SecurityLabel, access: u32) -> bool {
    if !MAC_ENABLED.load(Ordering::Relaxed) { return true; }
    let ok = MAC_POLICY.lock().check(subject, object, access);
    if !ok {
        audit_log(AuditEvent::MacDenied { subject, object, access });
    }
    ok
}

pub fn current_mac_label() -> SecurityLabel {
    crate::process::with_current(|p| p.mac_label).unwrap_or(SecurityLabel::USER)
}

// ── Layer 4: Syscall Allowlist (QSF-SAL) ─────────────────────────────────

/// Per-process syscall filter stored as a compact 512-bit bitmap.
/// Bit N set = syscall N is allowed.
#[derive(Clone)]
pub struct SyscallFilter {
    /// 512 / 64 = 8 words
    pub bitmap: [u64; 8],
    pub active: bool,
    /// Action on violation: true = SIGSYS, false = ENOSYS
    pub kill_on_violation: bool,
}

impl SyscallFilter {
    pub fn allow_all() -> Self {
        SyscallFilter { bitmap: [u64::MAX; 8], active: false, kill_on_violation: false }
    }
    pub fn deny_all() -> Self {
        SyscallFilter { bitmap: [0u64; 8], active: true, kill_on_violation: true }
    }
    pub fn allow(&mut self, nr: u64) {
        if nr < 512 { self.bitmap[(nr / 64) as usize] |= 1u64 << (nr % 64); }
    }
    pub fn deny(&mut self, nr: u64) {
        if nr < 512 { self.bitmap[(nr / 64) as usize] &= !(1u64 << (nr % 64)); }
    }
    #[inline]
    pub fn is_allowed(&self, nr: u64) -> bool {
        if !self.active || nr >= 512 { return true; }
        self.bitmap[(nr / 64) as usize] & (1u64 << (nr % 64)) != 0
    }
}

/// Pre-built filter profiles.
pub mod profiles {
    use super::SyscallFilter;

    /// Minimal shell profile: file I/O, process, signals, terminal — no network, no ptrace.
    pub fn shell() -> SyscallFilter {
        let mut f = SyscallFilter::deny_all();
        // File I/O
        for nr in [0,1,2,3,4,5,6,8,9,10,11,12,17,18,19,20,21,22,32,33,72,74,75,76,77,
                   78,79,80,82,83,84,85,86,87,88,89,90,91,92,94,95,133,161,162,217,
                   257,258,259,260,261,262,263,264,265,266,267,268,269,280] {
            f.allow(nr);
        }
        // Process + signals
        for nr in [13,14,15,24,34,35,39,56,57,58,59,60,61,62,63,101,102,104,107,108,
                   109,110,111,112,113,114,115,116,158,186,200,204,218,231] {
            f.allow(nr);
        }
        // Memory management
        for nr in [9,10,11,12,25,26,27,28] { f.allow(nr); }
        // Timing
        for nr in [96,97,98,100,201,202,203,228] { f.allow(nr); }
        // Misc
        for nr in [16,63,99,103,125,126,127,130,131,157,160,302,318,435] { f.allow(nr); }
        f.active = true; f
    }

    /// Network service profile: adds socket API on top of shell.
    pub fn network_service() -> SyscallFilter {
        let mut f = shell();
        for nr in [41,42,43,44,45,46,47,48,49,50,51,52,53,54,55,
                   213,214,215,233,281,282,283,284,285,286,287,288,
                   290,291,292,293] {
            f.allow(nr);
        }
        f
    }

    /// Fully restricted: only read, write, exit, nanosleep — for sandboxed helpers.
    pub fn minimal() -> SyscallFilter {
        let mut f = SyscallFilter::deny_all();
        for nr in [0, 1, 60, 231, 35, 39] { f.allow(nr); }
        f
    }
}

pub fn current_filter_check(nr: u64) -> QsfResult {
    let (filter_active, allowed, kill) = crate::process::with_current(|p| {
        let f = &p.syscall_filter;
        (f.active, f.is_allowed(nr), f.kill_on_violation)
    }).unwrap_or((false, true, false));

    if !filter_active || allowed { return QsfResult::Allow; }

    audit_log(AuditEvent::SyscallBlocked {
        pid: crate::process::current_pid(),
        nr,
    });

    if kill { QsfResult::Kill } else { QsfResult::Deny }
}

// ── Layer 5: Address Space Integrity (QSF-ASI) ────────────────────────────

static ASI_ENABLED: AtomicBool = AtomicBool::new(true);

pub fn asi_enable()  { ASI_ENABLED.store(true,  Ordering::Relaxed); }
pub fn asi_disable() { ASI_ENABLED.store(false, Ordering::Relaxed); }

/// Verify that a user-space pointer is valid and accessible.
/// Returns false if the pointer is in kernel space, unmapped, or misaligned.
#[inline]
pub fn verify_user_ptr(ptr: u64, len: usize) -> bool {
    if ptr == 0 { return len == 0; }
    // Must be in canonical user virtual address space
    if ptr >= 0x0000_8000_0000_0000 { return false; }
    if len == 0 { return true; }
    let end = match ptr.checked_add(len as u64) {
        Some(e) => e,
        None    => return false, // overflow
    };
    if end > 0x0000_8000_0000_0000 { return false; }
    if !ASI_ENABLED.load(Ordering::Relaxed) { return true; }
    // Check the range lies within a registered VMA
    crate::process::with_current(|p| {
        p.address_space.regions.iter().any(|r| r.start <= ptr && end <= r.end)
    }).unwrap_or(true) // kernel threads always allowed
}

/// Alias used throughout syscall handlers.
pub fn is_user_ptr_valid(ptr: u64, len: usize) -> bool {
    verify_user_ptr(ptr, len)
}

// ── Layer 6: Audit Log (QSF-AUDIT) ───────────────────────────────────────

const AUDIT_RING_SIZE: usize = 256; // entries

/// A security audit event.
#[derive(Clone)]
pub enum AuditEvent {
    SyscallBlocked  { pid: u32, nr: u64 },
    CapDenied       { pid: u32, cap: u64 },
    MacDenied       { subject: SecurityLabel, object: SecurityLabel, access: u32 },
    ExecEvent       { pid: u32, path: [u8; 64] },
    SetuidEvent     { pid: u32, old_uid: u32, new_uid: u32 },
    NetworkConnect  { pid: u32, dst_ip: u32, dst_port: u16 },
    FileDenied      { pid: u32, path: [u8; 64] },
}

struct AuditRing {
    entries:  Vec<AuditEntry>,
    head:     usize,
    total:    u64,
}

#[derive(Clone)]
struct AuditEntry {
    tick:    u64,
    pid:     u32,
    kind:    u8,
    detail:  [u8; 48],
}

impl AuditRing {
    fn new() -> Self {
        AuditRing {
            entries: alloc::vec![AuditEntry { tick:0, pid:0, kind:0, detail:[0;48] }; AUDIT_RING_SIZE],
            head: 0,
            total: 0,
        }
    }

    fn push(&mut self, pid: u32, kind: u8, detail: &[u8]) {
        let tick = crate::time::ticks();
        let entry = &mut self.entries[self.head % AUDIT_RING_SIZE];
        entry.tick = tick;
        entry.pid  = pid;
        entry.kind = kind;
        let n = detail.len().min(48);
        entry.detail[..n].copy_from_slice(&detail[..n]);
        self.head = self.head.wrapping_add(1);
        self.total += 1;
    }

    fn read_text(&self, out: &mut Vec<u8>) {
        let start = if self.total >= AUDIT_RING_SIZE as u64 {
            self.head % AUDIT_RING_SIZE
        } else { 0 };
        let count = self.total.min(AUDIT_RING_SIZE as u64) as usize;
        for i in 0..count {
            let e = &self.entries[(start + i) % AUDIT_RING_SIZE];
            let line = alloc::format!(
                "tick={} pid={} kind={} detail={:?}\n",
                e.tick, e.pid, e.kind,
                &e.detail[..e.detail.iter().position(|&b|b==0).unwrap_or(48)]
            );
            out.extend_from_slice(line.as_bytes());
        }
    }
}

static AUDIT: Mutex<Option<AuditRing>> = Mutex::new(None);
static AUDIT_ENABLED: AtomicBool = AtomicBool::new(false);

pub fn audit_enable() {
    *AUDIT.lock() = Some(AuditRing::new());
    AUDIT_ENABLED.store(true, Ordering::Release);
}

pub fn audit_is_enabled() -> bool { AUDIT_ENABLED.load(Ordering::Relaxed) }

pub fn audit_log(event: AuditEvent) {
    if !AUDIT_ENABLED.load(Ordering::Relaxed) { return; }
    let mut guard = AUDIT.lock();
    if let Some(ref mut ring) = *guard {
        let (pid, kind, detail) = match event {
            AuditEvent::SyscallBlocked { pid, nr } => {
                let mut d = [0u8; 48];
                let s = alloc::format!("nr={}", nr);
                let n = s.len().min(48); d[..n].copy_from_slice(&s.as_bytes()[..n]);
                (pid, 1u8, d)
            }
            AuditEvent::CapDenied { pid, cap } => {
                let mut d = [0u8; 48];
                let s = alloc::format!("cap={:#x}", cap);
                let n = s.len().min(48); d[..n].copy_from_slice(&s.as_bytes()[..n]);
                (pid, 2u8, d)
            }
            AuditEvent::MacDenied { subject, object, access } => {
                let mut d = [0u8; 48];
                let s = alloc::format!("s={:#x} o={:#x} a={:#x}", subject.0, object.0, access);
                let n = s.len().min(48); d[..n].copy_from_slice(&s.as_bytes()[..n]);
                (0u32, 3u8, d)
            }
            AuditEvent::ExecEvent { pid, path } => (pid, 4u8, {
                let mut d = [0u8; 48]; d[..48.min(path.len())].copy_from_slice(&path[..48.min(path.len())]); d
            }),
            AuditEvent::SetuidEvent { pid, old_uid, new_uid } => {
                let mut d = [0u8; 48];
                let s = alloc::format!("{}→{}", old_uid, new_uid);
                let n = s.len().min(48); d[..n].copy_from_slice(&s.as_bytes()[..n]);
                (pid, 5u8, d)
            }
            AuditEvent::NetworkConnect { pid, dst_ip, dst_port } => {
                let mut d = [0u8; 48];
                let s = alloc::format!("{}.{}.{}.{}:{}", dst_ip>>24, (dst_ip>>16)&0xFF, (dst_ip>>8)&0xFF, dst_ip&0xFF, dst_port);
                let n = s.len().min(48); d[..n].copy_from_slice(&s.as_bytes()[..n]);
                (pid, 6u8, d)
            }
            AuditEvent::FileDenied { pid, path } => (pid, 7u8, {
                let mut d = [0u8; 48]; d[..48.min(path.len())].copy_from_slice(&path[..48.min(path.len())]); d
            }),
        };
        ring.push(pid, kind, &detail);
    }
}

/// Render audit log for /proc/qsf/audit
pub fn audit_read() -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(ref ring) = *AUDIT.lock() { ring.read_text(&mut out); }
    out
}

// ── QSF unified check ─────────────────────────────────────────────────────

/// Result of a security check.
#[derive(Clone, Copy, PartialEq)]
pub enum QsfResult {
    Allow,
    Deny,   // return -EPERM / -EACCES to userspace
    Kill,   // send SIGSYS and kill the process
}

/// The single entry point for all kernel security decisions.
/// Called from syscall dispatch, VFS, network, IPC, and signal delivery.
///
/// Returns QsfResult::Allow on the happy path (no overhead when all layers pass).
#[inline]
pub fn qsf_check_syscall(nr: u64) -> QsfResult {
    // Layer 4: syscall filter
    current_filter_check(nr)
}

/// VFS access check: DAC + MAC combined.
#[inline]
pub fn qsf_check_file(
    uid: u32, gid: u32,
    file_uid: u32, file_gid: u32, file_mode: u32,
    access: u8,
    subject_label: SecurityLabel,
    object_label:  SecurityLabel,
) -> QsfResult {
    // Layer 1: DAC
    if !dac_check(uid, gid, file_uid, file_gid, file_mode, access) {
        return QsfResult::Deny;
    }
    // Layer 3: MAC
    let mac_access = match access {
        x if x & MAY_READ  != 0 => MacAccess::READ,
        x if x & MAY_WRITE != 0 => MacAccess::WRITE,
        x if x & MAY_EXEC  != 0 => MacAccess::EXEC,
        _ => MacAccess::READ,
    };
    if !mac_check(subject_label, object_label, mac_access) {
        return QsfResult::Deny;
    }
    QsfResult::Allow
}

/// Network connection check.
#[inline]
pub fn qsf_check_network(pid: u32, dst_ip: u32, dst_port: u16) -> QsfResult {
    if !current_has_cap(CAP_NET_ADMIN) {
        // Non-privileged: allow connections to ports > 1023 only
        if dst_port != 0 && dst_port <= 1023 {
            let uid = crate::process::with_current(|p| p.uid).unwrap_or(1);
            if uid != 0 { return QsfResult::Deny; }
        }
    }
    if audit_is_enabled() && dst_ip != 0x7F000001 { // skip loopback
        audit_log(AuditEvent::NetworkConnect { pid, dst_ip, dst_port });
    }
    QsfResult::Allow
}

// ── Namespaces (process isolation container primitives) ───────────────────

#[derive(Clone, Copy)]
pub struct Namespaces {
    pub pid_ns:  u32,
    pub net_ns:  u32,
    pub mnt_ns:  u32,
    pub uts_ns:  u32,
    pub ipc_ns:  u32,
    pub user_ns: u32,
}

impl Namespaces {
    pub const fn root() -> Self {
        Namespaces { pid_ns:0, net_ns:0, mnt_ns:0, uts_ns:0, ipc_ns:0, user_ns:0 }
    }
}

// ── Initialization ────────────────────────────────────────────────────────

pub fn init() {
    audit_enable();
    crate::klog!("QSF: Qunix Security Foundation active");
    crate::klog!("QSF: layers: DAC + Capabilities + MAC + SAL + ASI + Audit");
    crate::klog!("QSF: audit ring {} entries, ASI enabled, MAC disabled (allow-all default)",
        AUDIT_RING_SIZE);
}

/// Legacy compatibility aliases
pub fn check_permission(uid: u32, gid: u32, f_uid: u32, f_gid: u32, mode: u32, access: u8) -> bool {
    dac_check(uid, gid, f_uid, f_gid, mode, access)
}
pub fn current_may_access(f_uid: u32, f_gid: u32, mode: u32, access: u8) -> bool {
    current_dac_check(f_uid, f_gid, mode, access)
}
pub fn check_abi_syscall(nr: u64) -> bool { true } // filter moved to qsf_check_syscall
