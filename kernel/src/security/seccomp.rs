//! Seccomp-BPF — Classic BPF interpreter for seccomp syscall filtering.
//!
//! Implements the full cBPF (classic Berkeley Packet Filter) VM as used by
//! Linux seccomp. Programs written by userspace with `prctl(PR_SET_SECCOMP,
//! SECCOMP_MODE_FILTER, prog)` are loaded, validated, and stored per-process.
//! On every syscall, the VM runs the program against `seccomp_data` and maps
//! the return value to a QSF-SAL action.
//!
//! ## BPF VM internals
//!
//! Classic BPF operates on a fixed-width register machine:
//!   A  — accumulator (u32)
//!   X  — index register (u32)
//!   M  — scratch memory (16 × u32 words)
//!
//! Instructions are 8 bytes: opcode(u16) jt(u8) jf(u8) k(u32)
//!
//! ## seccomp_data layout (read-only input to the BPF program)
//!
//!   +0  nr      (u32)  — syscall number
//!   +4  arch    (u32)  — AUDIT_ARCH_X86_64 = 0xC000003E
//!   +8  ip      (u64)  — instruction pointer at syscall
//!   +16 args[6] (u64)  — syscall arguments
//!
//! ## Return value mapping
//!
//!   SECCOMP_RET_ALLOW   (0x7FFF_0000) → QsfResult::Allow
//!   SECCOMP_RET_KILL    (0x0000_0000) → send SIGSYS, kill thread
//!   SECCOMP_RET_TRAP    (0x0003_0000) → send SIGSYS (data in low bits)
//!   SECCOMP_RET_ERRNO   (0x0005_0000) → return -errno (low 16 bits)
//!   SECCOMP_RET_TRACE   (0x7FF0_0000) → QsfResult::Allow (ptrace not impl)
//!   SECCOMP_RET_LOG     (0x7FFC_0000) → log + allow
//!   SECCOMP_RET_USER_NOTIF (0x7FC0_0000) → allow (user_notif fd not impl)
//!
//! ## Validation
//!
//! Before storing a program, the validator checks:
//!   - Length: 1–4096 instructions
//!   - No out-of-bounds jumps
//!   - Last instruction must be a RET
//!   - Memory accesses only within M[0..15]
//!   - Load offsets within seccomp_data (64 bytes)

use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;
use crate::security::QsfResult;

// ── BPF constants ─────────────────────────────────────────────────────────

// Classes
const BPF_LD:   u16 = 0x00;
const BPF_LDX:  u16 = 0x01;
const BPF_ST:   u16 = 0x02;
const BPF_STX:  u16 = 0x03;
const BPF_ALU:  u16 = 0x04;
const BPF_JMP:  u16 = 0x05;
const BPF_RET:  u16 = 0x06;
const BPF_MISC: u16 = 0x07;

// Size
const BPF_W:    u16 = 0x00; // 32-bit word
const BPF_H:    u16 = 0x08; // 16-bit half-word
const BPF_B:    u16 = 0x10; // 8-bit byte

// Mode
const BPF_IMM:  u16 = 0x00;
const BPF_ABS:  u16 = 0x20;
const BPF_IND:  u16 = 0x40;
const BPF_MEM:  u16 = 0x60;
const BPF_LEN:  u16 = 0x80;
const BPF_MSH:  u16 = 0xa0;

// ALU operations
const BPF_ADD:  u16 = 0x00;
const BPF_SUB:  u16 = 0x10;
const BPF_MUL:  u16 = 0x20;
const BPF_DIV:  u16 = 0x30;
const BPF_OR:   u16 = 0x40;
const BPF_AND:  u16 = 0x50;
const BPF_LSH:  u16 = 0x60;
const BPF_RSH:  u16 = 0x70;
const BPF_NEG:  u16 = 0x80;
const BPF_MOD:  u16 = 0x90;
const BPF_XOR:  u16 = 0xa0;

// Jump ops
const BPF_JA:   u16 = 0x00;
const BPF_JEQ:  u16 = 0x10;
const BPF_JGT:  u16 = 0x20;
const BPF_JGE:  u16 = 0x30;
const BPF_JSET: u16 = 0x40;

// Jump sources
const BPF_K:    u16 = 0x00;
const BPF_X:    u16 = 0x08;

// RET sources
const BPF_A:    u16 = 0x10;

// MISC ops
const BPF_TAX:  u16 = 0x00;
const BPF_TXA:  u16 = 0x80;

// seccomp return codes
pub const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
pub const SECCOMP_RET_KILL_THREAD:  u32 = 0x0000_0000;
pub const SECCOMP_RET_KILL:         u32 = 0x0000_0000; // alias
pub const SECCOMP_RET_TRAP:         u32 = 0x0003_0000;
pub const SECCOMP_RET_ERRNO:        u32 = 0x0005_0000;
pub const SECCOMP_RET_USER_NOTIF:   u32 = 0x7FC0_0000;
pub const SECCOMP_RET_TRACE:        u32 = 0x7FF0_0000;
pub const SECCOMP_RET_LOG:          u32 = 0x7FFC_0000;
pub const SECCOMP_RET_ALLOW:        u32 = 0x7FFF_0000;
pub const SECCOMP_RET_ACTION_FULL:  u32 = 0xFFFF_0000;

// AUDIT_ARCH_X86_64
pub const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;

// seccomp_data offsets (bytes)
const OFF_NR:    u32 = 0;
const OFF_ARCH:  u32 = 4;
const OFF_IP:    u32 = 8;
const OFF_ARG0:  u32 = 16;
const SECCOMP_DATA_LEN: u32 = 64; // 4 + 4 + 8 + 6*8

// prctl constants
pub const PR_SET_SECCOMP:     i32 = 22;
pub const PR_GET_SECCOMP:     i32 = 21;
pub const SECCOMP_MODE_STRICT: u64 = 1;
pub const SECCOMP_MODE_FILTER: u64 = 2;

// seccomp(2) opcodes
pub const SECCOMP_SET_MODE_STRICT: u32 = 0;
pub const SECCOMP_SET_MODE_FILTER: u32 = 1;
pub const SECCOMP_GET_ACTION_AVAIL: u32 = 2;
pub const SECCOMP_FILTER_FLAG_LOG:  u32 = 1 << 1;
pub const SECCOMP_FILTER_FLAG_NEW_LISTENER: u32 = 1 << 3;

// ── BPF instruction ───────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct BpfInsn {
    pub code: u16,
    pub jt:   u8,
    pub jf:   u8,
    pub k:    u32,
}

impl BpfInsn {
    fn class(&self) -> u16 { self.code & 0x07 }
    fn size(&self)  -> u16 { self.code & 0x18 }
    fn mode(&self)  -> u16 { self.code & 0xe0 }
    fn op(&self)    -> u16 { self.code & 0xf0 }
    fn src(&self)   -> u16 { self.code & 0x08 }
}

/// Parsed + validated seccomp BPF program.
#[derive(Clone)]
pub struct SeccompFilter {
    pub insns: Vec<BpfInsn>,
    pub log:   bool,   // SECCOMP_FILTER_FLAG_LOG
}

// ── seccomp_data (input to BPF program) ──────────────────────────────────

pub struct SeccompData {
    pub nr:   u32,
    pub arch: u32,
    pub ip:   u64,
    pub args: [u64; 6],
}

impl SeccompData {
    pub fn new(nr: u64, frame_ip: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> Self {
        SeccompData {
            nr:   nr as u32,
            arch: AUDIT_ARCH_X86_64,
            ip:   frame_ip,
            args: [a0, a1, a2, a3, a4, a5],
        }
    }

    /// Read a u32 from the seccomp_data at a given byte offset.
    /// Used by BPF_LD|BPF_ABS and BPF_LD|BPF_IND.
    fn load_u32(&self, offset: u32) -> Option<u32> {
        match offset {
            0 => Some(self.nr),
            4 => Some(self.arch),
            8 | 9 | 10 | 11 => {
                let byte = ((self.ip >> (8 * (offset - 8))) & 0xFF) as u32;
                Some(byte)
            }
            8..=15 => {
                let shift = (offset - 8) * 8;
                if offset + 4 <= 16 {
                    Some((self.ip >> shift) as u32)
                } else { None }
            }
            16..=63 => {
                // args[0] at 16, args[1] at 24, …
                let arg_idx = ((offset - 16) / 8) as usize;
                if arg_idx >= 6 { return None; }
                let byte_off = (offset - 16) % 8;
                let val = (self.args[arg_idx] >> (byte_off * 8)) as u32;
                Some(val)
            }
            _ => None,
        }
    }
}

// ── Validator ─────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum BpfError {
    TooLong,
    Empty,
    InvalidJump { pc: usize },
    InvalidMemAccess { pc: usize },
    InvalidLoadOffset { pc: usize },
    NoReturnAtEnd,
    DivisionByConstantZero { pc: usize },
}

pub fn validate(insns: &[BpfInsn]) -> Result<(), BpfError> {
    const MAX_INSNS: usize = 4096;
    if insns.is_empty() { return Err(BpfError::Empty); }
    if insns.len() > MAX_INSNS { return Err(BpfError::TooLong); }

    for (pc, insn) in insns.iter().enumerate() {
        match insn.class() {
            BPF_LD | BPF_LDX => {
                match insn.mode() {
                    BPF_ABS | BPF_IND => {
                        // Offset must land within seccomp_data
                        // For BPF_ABS: k is the offset
                        // For BPF_IND: k is added to X at runtime, can't fully validate
                        if insn.mode() == BPF_ABS && insn.k + 4 > SECCOMP_DATA_LEN {
                            return Err(BpfError::InvalidLoadOffset { pc });
                        }
                    }
                    BPF_MEM => {
                        if insn.k >= 16 { return Err(BpfError::InvalidMemAccess { pc }); }
                    }
                    BPF_IMM | BPF_LEN | BPF_MSH => {}
                    _ => {}
                }
            }
            BPF_ST | BPF_STX => {
                if insn.k >= 16 { return Err(BpfError::InvalidMemAccess { pc }); }
            }
            BPF_ALU => {
                // Check for constant division/modulo by zero
                if (insn.op() == BPF_DIV || insn.op() == BPF_MOD)
                   && insn.src() == BPF_K && insn.k == 0 {
                    return Err(BpfError::DivisionByConstantZero { pc });
                }
            }
            BPF_JMP => {
                if insn.op() == BPF_JA {
                    let target = pc.wrapping_add(1 + insn.k as usize);
                    if target >= insns.len() {
                        return Err(BpfError::InvalidJump { pc });
                    }
                } else {
                    let jt_target = pc + 1 + insn.jt as usize;
                    let jf_target = pc + 1 + insn.jf as usize;
                    if jt_target >= insns.len() || jf_target >= insns.len() {
                        return Err(BpfError::InvalidJump { pc });
                    }
                }
            }
            _ => {}
        }
    }

    // Last instruction must be a return
    let last = &insns[insns.len() - 1];
    if last.class() != BPF_RET {
        return Err(BpfError::NoReturnAtEnd);
    }

    Ok(())
}

// ── VM interpreter ────────────────────────────────────────────────────────

const MEM_WORDS: usize = 16;

/// Execute a validated seccomp BPF program against `data`.
/// Returns the 32-bit return value (seccomp action code).
pub fn run(prog: &[BpfInsn], data: &SeccompData) -> u32 {
    let mut a:   u32     = 0;
    let mut x:   u32     = 0;
    let mut mem: [u32; MEM_WORDS] = [0; MEM_WORDS];
    let mut pc:  usize   = 0;

    // Safety limit: 1 million instructions max to prevent infinite loops
    let mut budget: u32 = 1_000_000;

    loop {
        if pc >= prog.len() || budget == 0 { return SECCOMP_RET_KILL; }
        budget -= 1;
        let insn = &prog[pc];

        match insn.class() {
            // ── LD: load into A ───────────────────────────────────────────
            BPF_LD => {
                a = match insn.mode() {
                    BPF_IMM => insn.k,
                    BPF_ABS => data.load_u32(insn.k).unwrap_or(0),
                    BPF_IND => data.load_u32(x.wrapping_add(insn.k)).unwrap_or(0),
                    BPF_MEM => mem.get(insn.k as usize).copied().unwrap_or(0),
                    BPF_LEN => SECCOMP_DATA_LEN,
                    _ => 0,
                };
            }
            // ── LDX: load into X ──────────────────────────────────────────
            BPF_LDX => {
                x = match insn.mode() {
                    BPF_IMM => insn.k,
                    BPF_MEM => mem.get(insn.k as usize).copied().unwrap_or(0),
                    BPF_LEN => SECCOMP_DATA_LEN,
                    BPF_MSH => 0, // IP header length — not meaningful in seccomp
                    _ => 0,
                };
            }
            // ── ST/STX: store from A or X ─────────────────────────────────
            BPF_ST  => { if (insn.k as usize) < MEM_WORDS { mem[insn.k as usize] = a; } }
            BPF_STX => { if (insn.k as usize) < MEM_WORDS { mem[insn.k as usize] = x; } }
            // ── ALU ───────────────────────────────────────────────────────
            BPF_ALU => {
                let src = if insn.src() == BPF_X { x } else { insn.k };
                a = match insn.op() {
                    BPF_ADD => a.wrapping_add(src),
                    BPF_SUB => a.wrapping_sub(src),
                    BPF_MUL => a.wrapping_mul(src),
                    BPF_DIV => if src != 0 { a / src } else { return SECCOMP_RET_KILL; }
                    BPF_MOD => if src != 0 { a % src } else { return SECCOMP_RET_KILL; }
                    BPF_OR  => a | src,
                    BPF_AND => a & src,
                    BPF_XOR => a ^ src,
                    BPF_LSH => a.checked_shl(src).unwrap_or(0),
                    BPF_RSH => a.checked_shr(src).unwrap_or(0),
                    BPF_NEG => (!a).wrapping_add(1),
                    _       => a,
                };
            }
            // ── JMP ───────────────────────────────────────────────────────
            BPF_JMP => {
                let src = if insn.src() == BPF_X { x } else { insn.k };
                if insn.op() == BPF_JA {
                    pc = pc.wrapping_add(1 + insn.k as usize);
                    continue;
                }
                let taken = match insn.op() {
                    BPF_JEQ  => a == src,
                    BPF_JGT  => a >  src,
                    BPF_JGE  => a >= src,
                    BPF_JSET => a &  src != 0,
                    _        => false,
                };
                pc += 1 + if taken { insn.jt as usize } else { insn.jf as usize };
                continue;
            }
            // ── RET ───────────────────────────────────────────────────────
            BPF_RET => {
                return if insn.src() == BPF_A { a } else { insn.k };
            }
            // ── MISC ─────────────────────────────────────────────────────
            BPF_MISC => {
                match insn.op() {
                    BPF_TAX => { x = a; }
                    BPF_TXA => { a = x; }
                    _       => {}
                }
            }
            _ => { return SECCOMP_RET_KILL; }
        }

        pc += 1;
    }
}

// ── seccomp_data → QsfResult mapping ─────────────────────────────────────

pub fn action_to_qsf(retval: u32) -> (QsfResult, i32) {
    let action = retval & SECCOMP_RET_ACTION_FULL;
    let data   = (retval & 0x0000_FFFF) as i32;

    match action {
        x if x == SECCOMP_RET_ALLOW  => (QsfResult::Allow,        0),
        x if x == SECCOMP_RET_KILL   => (QsfResult::Kill,          0),
        x if x == SECCOMP_RET_KILL_PROCESS => (QsfResult::Kill,   0),
        x if x == SECCOMP_RET_TRAP   => (QsfResult::Kill,          0),  // SIGSYS
        x if x == SECCOMP_RET_ERRNO  => (QsfResult::Deny,          data), // -errno
        x if x >= SECCOMP_RET_TRACE  => (QsfResult::Allow,         0),  // trace=allow
        x if x == SECCOMP_RET_LOG    => (QsfResult::Allow,         0),  // log+allow
        _                            => (QsfResult::Allow,         0),
    }
}

// ── Per-process filter chain ──────────────────────────────────────────────
//
// A process may install multiple filters (each `prctl(PR_SET_SECCOMP,...)`
// adds one). Filters compose: all must allow for the syscall to proceed.
// Filters are inherited by children and cannot be removed.

#[derive(Clone, Default)]
pub struct FilterChain {
    pub filters:  Vec<SeccompFilter>,
    pub mode:     u32,   // 0=off, 1=strict, 2=filter
    pub notif_fd: Option<u32>, // notify fd id if NEW_LISTENER was used
}

impl FilterChain {
    pub fn new() -> Self { FilterChain { filters: Vec::new(), mode: 0, notif_fd: None } }

    pub fn is_active(&self) -> bool { self.mode != 0 }

    pub fn run_chain(&self, data: &SeccompData) -> (QsfResult, i32) {
        if self.mode == SECCOMP_MODE_STRICT as u32 {
            // Strict mode: only read(0), write(1), exit(60,231), sigreturn(15)
            match data.nr {
                0 | 1 | 15 | 60 | 231 => return (QsfResult::Allow, 0),
                _ => return (QsfResult::Kill, 0),
            }
        }

        // Filter mode: run all filters; most restrictive wins
        let mut most_restrictive = SECCOMP_RET_ALLOW;
        for filter in &self.filters {
            let ret = run(&filter.insns, data);
            if filter.log {
                crate::security::audit_log(crate::security::AuditEvent::SyscallBlocked {
                    pid: crate::process::current_pid(),
                    nr:  data.nr as u64,
                });
            }
            // Lower return value = more restrictive
            if ret < most_restrictive { most_restrictive = ret; }
        }
        action_to_qsf(most_restrictive)
    }

    pub fn install(&mut self, filter: SeccompFilter) {
        self.mode = SECCOMP_MODE_FILTER as u32;
        self.filters.push(filter);
    }

    pub fn set_strict(&mut self) {
        self.mode = SECCOMP_MODE_STRICT as u32;
    }
}

// ── syscall interface: seccomp(2) and prctl(PR_SET_SECCOMP) ──────────────

/// Load a seccomp BPF program from userspace.
/// `prog_ptr` points to a `sock_fprog` struct: { u16 len, u16 pad[3], *BpfInsn }.
unsafe fn load_user_prog(prog_ptr: u64) -> Result<Vec<BpfInsn>, i64> {
    if prog_ptr == 0 { return Err(-22); }
    let len = *(prog_ptr as *const u16) as usize;
    if len == 0 || len > 4096 { return Err(-22); }
    let insn_ptr = *((prog_ptr + 8) as *const u64); // sock_fprog.filter
    if insn_ptr == 0 { return Err(-22); }
    let insns: Vec<BpfInsn> = (0..len).map(|i| {
        *((insn_ptr + i as u64 * 8) as *const BpfInsn)
    }).collect();
    Ok(insns)
}

pub fn sys_seccomp_real(op: u32, flags: u32, args: u64) -> i64 {
    match op {
        SECCOMP_SET_MODE_STRICT => {
            crate::process::with_current_mut(|p| {
                p.seccomp.set_strict();
            });
            0
        }
        SECCOMP_SET_MODE_FILTER => {
            let insns = unsafe { match load_user_prog(args) {
                Ok(i)  => i,
                Err(e) => return e,
            }};
            match validate(&insns) {
                Err(_) => return -22, // EINVAL
                Ok(()) => {}
            }
            let log      = flags & SECCOMP_FILTER_FLAG_LOG != 0;
            let listener = flags & SECCOMP_FILTER_FLAG_NEW_LISTENER != 0;
            let filter   = SeccompFilter { insns, log };
            crate::process::with_current_mut(|p| {
                p.seccomp.install(filter);
            });
            if listener {
                // Create notification fd and return it as the syscall return value
                let fd_id = create_notif_fd();
                // Allocate a real file descriptor in the process
                let vfd = crate::process::with_current_mut(|p| {
                    use crate::vfs::{FileDescriptor, FdKind, Inode};
                    let f = FileDescriptor {
                        inode: Inode {
                            ino: fd_id as u64, mode: 0xC000, uid: 0, gid: 0, size: 0,
                            atime: 0, mtime: 0, ctime: 0,
                            ops: crate::vfs::DummyInodeOps::new(),
                            sb:  alloc::sync::Arc::new(crate::vfs::Superblock { dev: 0, fs_type: alloc::string::String::new(), ops: crate::vfs::DummySuperblock::new() }),
                        },
                        kind:  FdKind::SeccompNotif(fd_id),
                        flags: 0,
                        offset: 0,
                     path: alloc::string::String::new(),};
                    p.alloc_fd(f) as i32
                }).unwrap_or(-1);
                // Link fd_id to the process's seccomp chain
                crate::process::with_current_mut(|p| {
                    p.seccomp.notif_fd = Some(fd_id);
                });
                return vfd as i64;
            }
            0
        }
        SECCOMP_GET_ACTION_AVAIL => {
            // Tell userspace which return values we support
            let supported = [SECCOMP_RET_KILL, SECCOMP_RET_KILL_PROCESS,
                             SECCOMP_RET_TRAP, SECCOMP_RET_ERRNO,
                             SECCOMP_RET_TRACE, SECCOMP_RET_LOG,
                             SECCOMP_RET_ALLOW];
            if args != 0 && supported.contains(&(args as u32)) { 0 } else { -22 }
        }
        _ => -22, // EINVAL
    }
}

pub fn sys_prctl_seccomp(mode: u64, prog_ptr: u64) -> i64 {
    match mode {
        SECCOMP_MODE_STRICT => {
            crate::process::with_current_mut(|p| { p.seccomp.set_strict(); });
            0
        }
        SECCOMP_MODE_FILTER => {
            sys_seccomp_real(SECCOMP_SET_MODE_FILTER, 0, prog_ptr)
        }
        _ => -22,
    }
}

/// Hot path: check seccomp filter chain for the current process.
/// Called from QSF before every syscall dispatch.
#[inline]
pub fn check_seccomp(nr: u64, ip: u64, args: [u64; 6]) -> (QsfResult, i32) {
    let active = crate::process::with_current(|p| p.seccomp.is_active()).unwrap_or(false);
    if !active { return (QsfResult::Allow, 0); }

    let data = SeccompData {
        nr: nr as u32, arch: AUDIT_ARCH_X86_64, ip,
        args,
    };

    crate::process::with_current(|p| p.seccomp.run_chain(&data))
        .unwrap_or((QsfResult::Allow, 0))
}

// ── seccomp notification fd (SECCOMP_FILTER_FLAG_NEW_LISTENER) ────────────
//
// When a seccomp filter is installed with NEW_LISTENER, the kernel returns
// a notification fd. Userspace reads seccomp_notif structs from it, decides
// whether to allow/deny, and writes seccomp_notif_resp structs back.
//
// Implementation: backed by a kernel-side IPC pipe. The kernel writes notifs
// when a supervised syscall is intercepted; the supervisor reads and responds.
//
// On-disk ABI (from linux/seccomp.h):
//   struct seccomp_notif      { id, pid, flags, data:{nr,arch,ip,args} }
//   struct seccomp_notif_resp { id, val, error, flags }
//
// ioctls on the notify fd:
//   SECCOMP_IOCTL_NOTIF_RECV   (0xC0 | 00 << 8) → read one pending notif
//   SECCOMP_IOCTL_NOTIF_SEND   (0xC0 | 01 << 8) → send a response
//   SECCOMP_IOCTL_NOTIF_ID_VALID (0xC0 | 02 << 8) → check if notif ID valid

use alloc::collections::VecDeque;

pub const SECCOMP_IOCTL_NOTIF_RECV:     u64 = 0xC050_0000; // _IOWR magic
pub const SECCOMP_IOCTL_NOTIF_SEND:     u64 = 0xC018_0001;
pub const SECCOMP_IOCTL_NOTIF_ID_VALID: u64 = 0x4008_0002;

pub const SECCOMP_USER_NOTIF_FLAG_CONTINUE: u32 = 1 << 0;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SeccompNotif {
    pub id:    u64,
    pub pid:   u32,
    pub flags: u32,
    pub data:  SeccompNotifData,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SeccompNotifData {
    pub nr:   u32,
    pub arch: u32,
    pub ip:   u64,
    pub args: [u64; 6],
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SeccompNotifResp {
    pub id:    u64,
    pub val:   i64,
    pub error: i32,
    pub flags: u32,
}

/// A pending notification waiting for supervisor response.
struct PendingNotif {
    id:       u64,
    pid:      u32,
    notif:    SeccompNotif,
    /// Channel to send the response back to the blocked syscall.
    /// None means the notif was sent but response not yet received.
    responded: bool,
    response:  Option<SeccompNotifResp>,
}

/// The in-kernel state for one seccomp notification fd.
pub struct NotifState {
    pub fd_id:   u32,
    pending:     VecDeque<PendingNotif>,
    next_id:     u64,
}

impl NotifState {
    pub fn new(fd_id: u32) -> Self {
        NotifState { fd_id, pending: VecDeque::new(), next_id: 1 }
    }

    /// Called from seccomp filter when a notif-intercepted syscall hits.
    /// Returns the notification ID. The calling thread blocks until responded.
    pub fn push_notif(&mut self, pid: u32, data: &SeccompData) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let notif = SeccompNotif {
            id,
            pid,
            flags: 0,
            data: SeccompNotifData {
                nr: data.nr, arch: data.arch, ip: data.ip, args: data.args,
            },
        };
        self.pending.push_back(PendingNotif { id, pid, notif, responded: false, response: None });
        id
    }

    /// Supervisor reads a pending notification.
    pub fn recv_notif(&mut self) -> Option<SeccompNotif> {
        self.pending.front().map(|p| p.notif)
    }

    /// Supervisor sends a response.
    pub fn send_response(&mut self, resp: SeccompNotifResp) -> bool {
        for n in &mut self.pending {
            if n.id == resp.id {
                n.responded = true;
                n.response  = Some(resp);
                return true;
            }
        }
        false
    }

    /// Check if a notification ID is still valid (not yet responded to).
    pub fn id_valid(&self, id: u64) -> bool {
        self.pending.iter().any(|n| n.id == id && !n.responded)
    }

    /// Get response for a completed notification.
    pub fn get_response(&mut self, id: u64) -> Option<SeccompNotifResp> {
        if let Some(pos) = self.pending.iter().position(|n| n.id == id && n.responded) {
            let n = self.pending.remove(pos)?;
            return n.response;
        }
        None
    }
}

// duplicate Mutex import removed
use alloc::collections::BTreeMap;

static NOTIF_STATES: Mutex<BTreeMap<u32, NotifState>> = Mutex::new(BTreeMap::new());
static NEXT_NOTIF_FD: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(20_000);

/// Create a new seccomp notification fd. Returns the fd number.
pub fn create_notif_fd() -> u32 {
    let fd_id = NEXT_NOTIF_FD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    NOTIF_STATES.lock().insert(fd_id, NotifState::new(fd_id));
    fd_id
}

/// Called from the seccomp VM when a SECCOMP_RET_USER_NOTIF action fires.
/// Sends a notification to the supervisor and blocks until response arrives.
/// Returns the QsfResult from the supervisor decision.
pub fn intercept_for_notif(notif_fd: u32, pid: u32, data: &SeccompData) -> (QsfResult, i32) {
    let notif_id = {
        let mut g = NOTIF_STATES.lock();
        if let Some(ns) = g.get_mut(&notif_fd) {
            ns.push_notif(pid, data)
        } else {
            return (QsfResult::Allow, 0);
        }
    };

    // Busy-wait (with yield) for the supervisor to respond.
    // In a real kernel this would be a wait queue. Here we use a bounded spin.
    let mut attempts = 0u32;
    loop {
        {
            let mut g = NOTIF_STATES.lock();
            if let Some(ns) = g.get_mut(&notif_fd) {
                if let Some(resp) = ns.get_response(notif_id) {
                    if resp.flags & SECCOMP_USER_NOTIF_FLAG_CONTINUE != 0 {
                        return (QsfResult::Allow, 0);
                    }
                    if resp.error != 0 {
                        return (QsfResult::Deny, -resp.error);
                    }
                    return (QsfResult::Allow, 0);
                }
            }
        }
        attempts += 1;
        if attempts > 10_000 { break; } // give up after 10k polls
        crate::sched::yield_current();
    }
    // Timeout: allow (conservative)
    (QsfResult::Allow, 0)
}

/// Handle ioctl() calls on a seccomp notify fd.
pub fn notif_fd_ioctl(fd_id: u32, request: u64, arg: u64) -> i64 {
    match request {
        SECCOMP_IOCTL_NOTIF_RECV => {
            // Copy next pending notif into user buffer
            let mut g = NOTIF_STATES.lock();
            if let Some(ns) = g.get_mut(&fd_id) {
                if let Some(notif) = ns.recv_notif() {
                    if arg != 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                &notif as *const SeccompNotif as *const u8,
                                arg as *mut u8,
                                core::mem::size_of::<SeccompNotif>(),
                            );
                        }
                    }
                    return 0;
                }
            }
            -11 // EAGAIN — no pending notifs
        }
        SECCOMP_IOCTL_NOTIF_SEND => {
            if arg == 0 { return -22; }
            let resp: SeccompNotifResp = unsafe { core::ptr::read(arg as *const SeccompNotifResp) };
            let mut g = NOTIF_STATES.lock();
            if let Some(ns) = g.get_mut(&fd_id) {
                if ns.send_response(resp) { 0 } else { -22 } // EINVAL
            } else { -9 } // EBADF
        }
        SECCOMP_IOCTL_NOTIF_ID_VALID => {
            if arg == 0 { return -22; }
            let id: u64 = unsafe { core::ptr::read(arg as *const u64) };
            let g = NOTIF_STATES.lock();
            if let Some(ns) = g.get(&fd_id) {
                if ns.id_valid(id) { 0 } else { -22 }
            } else { -9 }
        }
        _ => -22,
    }
}
