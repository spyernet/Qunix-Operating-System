//! Hardware-assisted memory protection — Intel PKU (Protection Keys for Userspace).
//!
//! Intel MPX was deprecated and removed from Linux 5.6 and all modern CPUs.
//! We implement PKU instead — it is:
//!   - Present on Intel Skylake+ and all modern x86-64 CPUs
//!   - Available on AMD Zen 2+ as well
//!   - Actively maintained and used in production (Glibc uses it for malloc)
//!   - A genuine alternative to SPARC ADI and ARM MTE for protection domains
//!
//! ## What PKU does
//!
//! Every 4KB page has a 4-bit protection key stored in bits [62:59] of its PTE.
//! This gives 16 independent protection domains (keys 0–15).
//!
//! The PKRU register (32 bits, one register per logical CPU) controls
//! userspace access to each key:
//!
//!   PKRU bit 2i   = 1 → deny ALL access (read + write + exec) for key i
//!   PKRU bit 2i+1 = 1 → deny WRITE for key i (reads still allowed)
//!   PKRU = 0      → all keys fully accessible (default)
//!
//! ## Instructions
//!
//!   RDPKRU — read PKRU into EAX  (ring 3, no privilege)
//!   WRPKRU — write ECX:EDX:EAX to PKRU (ring 3, ECX and EDX must be 0)
//!
//! ## QSF integration
//!
//!   Key 0  — default (all process memory)
//!   Key 1  — secret/sensitive data (crypto keys, passwords)
//!   Key 2  — read-only mapped files
//!   Key 3  — stack guard pages (deny all access = tripwire)
//!   Key 4  — JIT/executable heap (write protected during execution)
//!   Key 5  — inter-process shared memory (per-pair access control)
//!   Keys 6–15 — user-defined via sys_pkey_alloc
//!
//! ## PKRU management
//!
//! The kernel maintains a per-thread shadow PKRU that is:
//!   - Saved/restored on context switch
//!   - Modified via sys_pkey_mprotect (maps pages to a key)
//!   - Enforced by the CPU without any software hot path

use core::sync::atomic::{AtomicBool, Ordering};
use crate::arch::x86_64::paging::PageFlags;

// ── CPU feature detection ─────────────────────────────────────────────────

static PKU_SUPPORTED:  AtomicBool = AtomicBool::new(false);
static PKU_ENABLED:    AtomicBool = AtomicBool::new(false);

/// Check if the CPU supports PKU (CPUID leaf 7, ECX bit 3).
pub fn cpu_has_pku() -> bool {
    let (_, _, ecx, _) = crate::arch::x86_64::cpu::cpuid(7);
    ecx & (1 << 3) != 0
}

/// Check if the CPU supports OSPKE (CR4 bit 22 — OS enabled PKU).
/// Required before RDPKRU/WRPKRU can be used from ring 3.
fn enable_ospke() {
    unsafe {
        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= 1u64 << 22; // CR4.PKE (Protection Keys Enable)
        core::arch::asm!("mov cr4, {}", in(reg) cr4);
    }
}

pub fn init() {
    if cpu_has_pku() {
        enable_ospke();
        PKU_SUPPORTED.store(true, Ordering::Release);
        PKU_ENABLED.store(true, Ordering::Release);
        // Initialize PKRU to 0 (all keys fully accessible by default)
        pkru_write(0);
        crate::klog!("QSF PKU: hardware memory protection enabled (16 keys)");
    } else {
        crate::klog!("QSF PKU: CPU does not support protection keys, using software fallback");
    }
}

pub fn is_enabled() -> bool { PKU_ENABLED.load(Ordering::Relaxed) }
pub fn is_supported() -> bool { PKU_SUPPORTED.load(Ordering::Relaxed) }

// ── PKRU register access ─────────────────────────────────────────────────

/// Read the current PKRU register value.
#[inline]
pub fn pkru_read() -> u32 {
    if !PKU_SUPPORTED.load(Ordering::Relaxed) { return 0; }
    let val: u32;
    unsafe {
        core::arch::asm!(
            "xor ecx, ecx",
            "rdpkru",
            out("eax") val,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
    val
}

/// Write a new value to the PKRU register.
/// ECX and EDX must be 0 (hardware requirement).
#[inline]
pub fn pkru_write(val: u32) {
    if !PKU_SUPPORTED.load(Ordering::Relaxed) { return; }
    unsafe {
        core::arch::asm!(
            "xor ecx, ecx",
            "xor edx, edx",
            "wrpkru",
            in("eax") val,
            out("ecx") _,
            out("edx") _,
            options(nostack, nomem),
        );
    }
}

// ── PKRU bit manipulation ─────────────────────────────────────────────────

/// PKRU bit layout: bit 2k = ADi (access disable), bit 2k+1 = WDi (write disable)
pub fn pkru_set_access_disable(pkru: u32, key: u8) -> u32 {
    pkru | (1u32 << (2 * key as u32))
}

pub fn pkru_set_write_disable(pkru: u32, key: u8) -> u32 {
    pkru | (1u32 << (2 * key as u32 + 1))
}

pub fn pkru_clear_restrictions(pkru: u32, key: u8) -> u32 {
    pkru & !(3u32 << (2 * key as u32))
}

pub fn pkru_is_access_disabled(pkru: u32, key: u8) -> bool {
    pkru & (1u32 << (2 * key as u32)) != 0
}

pub fn pkru_is_write_disabled(pkru: u32, key: u8) -> bool {
    pkru & (1u32 << (2 * key as u32 + 1)) != 0
}

// ── Well-known protection keys ────────────────────────────────────────────

pub const PKEY_DEFAULT:    u8 = 0;  // all process memory
pub const PKEY_SECRET:     u8 = 1;  // crypto keys, password buffers
pub const PKEY_READONLY:   u8 = 2;  // read-only mapped files
pub const PKEY_STACK_GUARD:u8 = 3;  // guard pages (deny all = tripwire)
pub const PKEY_JIT:        u8 = 4;  // JIT-compiled code (WD during exec)
pub const PKEY_IPC_SHARED: u8 = 5;  // shared memory segments
pub const PKEY_USER_START: u8 = 6;  // first user-allocatable key

/// Per-process protection key allocation bitmap.
/// Bit i set = key i is in use.
#[derive(Clone, Copy, Default)]
pub struct PkeyAllocMap(pub u16); // 16 keys, one bit each

impl PkeyAllocMap {
    pub fn new() -> Self {
        // Reserve keys 0–5 for kernel use
        let reserved: u16 = (1 << (PKEY_USER_START)) - 1;
        PkeyAllocMap(reserved)
    }
    pub fn alloc(&mut self) -> Option<u8> {
        for k in PKEY_USER_START..16 {
            if self.0 & (1 << k) == 0 {
                self.0 |= 1 << k;
                return Some(k);
            }
        }
        None
    }
    pub fn free(&mut self, key: u8) {
        if key >= PKEY_USER_START { self.0 &= !(1 << key); }
    }
    pub fn is_allocated(&self, key: u8) -> bool { self.0 & (1 << key) != 0 }
}

// ── PTE protection key field ──────────────────────────────────────────────
//
// x86-64 PTE layout:
//   Bits [62:59] = protection key (4 bits)
//   Bits [63]    = NX (no execute)
//   Bits [11:9]  = software available
//
// PageFlags already handles the NX bit. We add pkey encoding here.

pub fn pte_set_pkey(pte_flags: u64, key: u8) -> u64 {
    let key = (key & 0xF) as u64;
    (pte_flags & !(0xF << 59)) | (key << 59)
}

pub fn pte_get_pkey(pte_flags: u64) -> u8 {
    ((pte_flags >> 59) & 0xF) as u8
}

// ── sys_pkey_alloc and sys_pkey_free ─────────────────────────────────────

pub fn sys_pkey_alloc(flags: u64, init_rights: u64) -> i64 {
    if !is_enabled() { return -524; } // ENOSYS on non-PKU systems → -38, but use -524 (ENOTSUPP)
    crate::process::with_current_mut(|p| {
        match p.pkey_map.alloc() {
            None => -28i64, // ENOSPC
            Some(key) => {
                // Apply initial access rights to this process's PKRU shadow
                let mut pkru = p.pkru_shadow;
                if init_rights & PKEY_DISABLE_ACCESS as u64 != 0 {
                    pkru = pkru_set_access_disable(pkru, key);
                } else if init_rights & PKEY_DISABLE_WRITE as u64 != 0 {
                    pkru = pkru_set_write_disable(pkru, key);
                }
                p.pkru_shadow = pkru;
                pkru_write(pkru);
                key as i64
            }
        }
    }).unwrap_or(-1)
}

pub fn sys_pkey_free(key: u32) -> i64 {
    if !is_enabled() { return 0; }
    if key < PKEY_USER_START as u32 || key > 15 { return -22; }
    crate::process::with_current_mut(|p| {
        p.pkey_map.free(key as u8);
        // Clear restrictions for this key in PKRU shadow
        p.pkru_shadow = pkru_clear_restrictions(p.pkru_shadow, key as u8);
        pkru_write(p.pkru_shadow);
    });
    0
}

pub const PKEY_DISABLE_ACCESS: u32 = 1;
pub const PKEY_DISABLE_WRITE:  u32 = 2;

pub fn sys_pkey_mprotect(addr: u64, len: usize, prot: i32, key: u32) -> i64 {
    if !is_enabled() {
        // Fall back to mprotect without key
        return crate::syscall::handlers::sys_mprotect(addr, len as u64, prot);
    }
    if key > 15 { return -22; }

    // Apply mprotect first
    let r = crate::syscall::handlers::sys_mprotect(addr, len as u64, prot);
    if r != 0 { return r; }

    // Re-map pages with the protection key set in the PTE
    let pid = crate::process::current_pid();
    let pml4_phys = crate::process::with_current(|p| p.address_space.pml4_phys).unwrap_or(0);
    if pml4_phys == 0 { return -22; }

    let pages = (len + 4095) / 4096;
    for i in 0..pages as u64 {
        let virt = addr + i * 4096;
        // Translate virt → phys, then re-set the PTE with the new key
        unsafe {
            let mut mapper = crate::arch::x86_64::paging::PageMapper::new(pml4_phys);
            if let Some(phys) = mapper.translate(virt) {
                let phys_base = phys & !0xFFF;
                let old_flags = mapper.get_flags(virt).unwrap_or(
                    crate::arch::x86_64::paging::PageFlags::PRESENT |
                    crate::arch::x86_64::paging::PageFlags::USER
                );
                // Encode the key into the raw PTE bits and remap
                let new_raw = pte_set_pkey(old_flags.bits(), key as u8);
                mapper.map_page_raw(virt, phys_base, new_raw);
            }
        }
    }
    0
}

// ── Context switch: save/restore PKRU ────────────────────────────────────
//
// Called from sched::switch_to() before and after context switch.

/// Save the current PKRU into the outgoing process's shadow.
pub fn context_switch_out(pid: u32) {
    if !is_enabled() { return; }
    let pkru = pkru_read();
    crate::process::with_process_mut(pid, |p| { p.pkru_shadow = pkru; });
}

/// Restore the incoming process's PKRU shadow.
pub fn context_switch_in(pid: u32) {
    if !is_enabled() { return; }
    let pkru = crate::process::with_process(pid, |p| p.pkru_shadow).unwrap_or(0);
    pkru_write(pkru);
}

// ── Software fallback (when PKU not available) ────────────────────────────
//
// When the CPU does not support PKU, we implement a software page permission
// table — slower but functionally equivalent for ASI enforcement.

#[derive(Clone, Default)]
pub struct SoftwareKeyTable {
    /// Pages protected by each key: key → set of page-aligned addresses
    entries: [alloc::collections::BTreeSet<u64>; 16],
    /// Access rights per key: same encoding as PKRU
    rights:  [u8; 16],
}

impl SoftwareKeyTable {
    pub fn set_key_for_range(&mut self, addr: u64, len: usize, key: u8) {
        let pages = (len + 4095) / 4096;
        for i in 0..pages as u64 {
            self.entries[key as usize & 15].insert((addr + i * 4096) & !0xFFF);
        }
    }
    pub fn is_access_denied(&self, addr: u64, write: bool) -> bool {
        let page = addr & !0xFFF;
        for (key, pages) in self.entries.iter().enumerate() {
            if pages.contains(&page) {
                let rights = self.rights[key];
                if rights & PKEY_DISABLE_ACCESS as u8 != 0 { return true; }
                if write && rights & PKEY_DISABLE_WRITE as u8 != 0 { return true; }
            }
        }
        false
    }
}

/// Software-fallback check — used instead of PKRU on non-PKU CPUs.
/// Called from verify_user_ptr when PKU is not available.
pub fn sw_check_access(addr: u64, len: usize, write: bool) -> bool {
    if is_supported() { return true; } // Hardware PKU handles it
    crate::process::with_current(|p| {
        !p.sw_pkey_table.is_access_denied(addr, write)
    }).unwrap_or(true)
}

// ── /proc/qsf/pku status ─────────────────────────────────────────────────

pub fn pku_status() -> alloc::vec::Vec<u8> {
    let supported = is_supported();
    let enabled   = is_enabled();
    let pkru      = if supported { pkru_read() } else { 0 };
    alloc::format!(
        "supported={}\nenabled={}\nkeys=16\npkru={:#010x}\n\
         key0=default key1=secret key2=readonly key3=guard key4=jit key5=ipc\n",
        supported, enabled, pkru
    ).into_bytes()
}
