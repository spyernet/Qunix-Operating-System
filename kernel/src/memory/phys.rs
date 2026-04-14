/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Physical frame allocator — per-CPU magazine + global buddy.
//!
//! Architecture:
//!   1. Per-CPU magazine cache (lock-free on the fast path)
//!      - Each CPU has a "loaded" magazine (up to MAGAZINE_SIZE frames)
//!      - alloc_frame() pops from loaded magazine (atomic pop, no global lock)
//!      - free_frame()  pushes to loaded magazine
//!      - Magazine exchange (empty↔full) touches the global layer
//!
//!   2. Global buddy allocator (power-of-two block sizes, order 0..=MAX_ORDER)
//!      - Splits and merges blocks; protected by a single spinlock
//!      - Magazines refill from buddy; buddy reclaims drained magazines
//!
//! Why this is faster than Linux slab on the alloc_frame() hot path:
//!   Linux free_pages() acquires a per-zone spinlock every call.
//!   Ours is a LOCK-FREE pop from a per-CPU atomic stack on the fast path.
//!   The global lock is only touched ~every MAGAZINE_SIZE allocations.

use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use crate::boot::{BootInfo, UefiMemoryDescriptor, uefi_memory_type};
use crate::arch::x86_64::paging::PAGE_SIZE;

// ── Constants ────────────────────────────────────────────────────────────

const MAX_FRAMES:    usize = 1 << 20; // 4 GB / 4 KB
const BITMAP_WORDS:  usize = MAX_FRAMES / 64;
const MAX_ORDER:     usize = 11;      // 2^11 pages = 8 MB
const MAGAZINE_SIZE: usize = 64;      // frames per CPU magazine
const MAX_CPUS_PHYS: usize = 64;

// ── Buddy allocator ───────────────────────────────────────────────────────

/// Buddy free list per order.
/// Uses a fixed-size bitmap for each order; free blocks tracked as bit=1.
struct BuddyAlloc {
    /// Bitmap: bit set = block at that index is free.
    /// For order n, index i represents the block starting at frame i * 2^n.
    free_bitmaps: [[u64; BITMAP_WORDS]; MAX_ORDER + 1],
    total_frames: usize,
    free_frames:  usize,
}

impl BuddyAlloc {
    const fn new() -> Self {
        BuddyAlloc {
            free_bitmaps: [[0u64; BITMAP_WORDS]; MAX_ORDER + 1],
            total_frames: 0,
            free_frames:  0,
        }
    }

    fn set_free(&mut self, order: usize, block_idx: usize) {
        self.free_bitmaps[order][block_idx / 64] |= 1u64 << (block_idx % 64);
    }

    fn clr_free(&mut self, order: usize, block_idx: usize) {
        self.free_bitmaps[order][block_idx / 64] &= !(1u64 << (block_idx % 64));
    }

    fn is_free(&self, order: usize, block_idx: usize) -> bool {
        self.free_bitmaps[order][block_idx / 64] & (1u64 << (block_idx % 64)) != 0
    }

    /// Find the lowest free block at the given order.
    fn find_free(&self, order: usize) -> Option<usize> {
        let max = (MAX_FRAMES >> order) / 64 + 1;
        for i in 0..max.min(BITMAP_WORDS) {
            let w = self.free_bitmaps[order][i];
            if w != 0 {
                return Some(i * 64 + w.trailing_zeros() as usize);
            }
        }
        None
    }

    /// Allocate 2^order contiguous frames. Returns physical address.
    fn alloc(&mut self, order: usize) -> Option<u64> {
        // Find a free block at this order or higher
        for o in order..=MAX_ORDER {
            if let Some(idx) = self.find_free(o) {
                self.clr_free(o, idx);
                // Split down to the requested order
                let mut current_idx = idx;
                for split_order in (order..o).rev() {
                    // Right half is free at split_order
                    let buddy_idx = current_idx * 2 + 1;
                    self.set_free(split_order, buddy_idx);
                    current_idx *= 2;
                }
                let frames = 1usize << order;
                if self.free_frames >= frames { self.free_frames -= frames; }
                return Some(current_idx as u64 * frames as u64 * PAGE_SIZE);
            }
        }
        None
    }

    /// Free 2^order contiguous frames, attempting to coalesce with buddy.
    fn free(&mut self, phys: u64, order: usize) {
        let frames      = 1usize << order;
        let block_idx   = (phys / PAGE_SIZE) as usize / frames;
        self.free_frames += frames;

        let mut idx = block_idx;
        let mut o   = order;
        loop {
            let buddy = idx ^ 1; // buddy is the adjacent block at the same order
            if o >= MAX_ORDER || !self.is_free(o, buddy) {
                self.set_free(o, idx);
                break;
            }
            // Coalesce with buddy
            self.clr_free(o, buddy);
            idx = idx / 2; // merged block index at next order
            o  += 1;
        }
    }

    /// Mark a range of frames as used (reserved).
    fn mark_range_used(&mut self, first: usize, count: usize) {
        for f in first..first + count {
            let block = f; // order=0 block index
            self.clr_free(0, block);
        }
        if count <= self.free_frames { self.free_frames -= count; }
    }

    /// Add a range of frames as available memory.
    fn add_range(&mut self, first: usize, count: usize) {
        // Add frame by frame at order=0, then let future alloc/free coalesce
        for f in first..first + count {
            self.free(f as u64 * PAGE_SIZE, 0);
        }
        self.total_frames += count;
    }
}

static BUDDY: Mutex<BuddyAlloc> = Mutex::new(BuddyAlloc::new());

// ── Per-CPU magazine cache ─────────────────────────────────────────────────
//
// Layout of each magazine: a fixed stack of physical addresses.
// The "top" pointer is an atomic so we can pop without a lock if the CPU
// is single-threaded (which kernel fast paths are).
// Exchange operations (refill/drain) still need the buddy lock.

#[repr(C, align(64))]
struct Magazine {
    frames: [u64; MAGAZINE_SIZE],
    top:    AtomicUsize,  // index of next free slot (0 = empty, MAGAZINE_SIZE = full)
}

impl Magazine {
    const fn new() -> Self {
        Magazine { frames: [0u64; MAGAZINE_SIZE], top: AtomicUsize::new(0) }
    }

    /// Attempt a lock-free pop. Returns physical address or 0 on empty.
    #[inline]
    fn pop(&self) -> u64 {
        loop {
            let t = self.top.load(Ordering::Acquire);
            if t == 0 { return 0; }
            // CAS to claim this slot
            if self.top.compare_exchange(t, t - 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                let phys = unsafe { *self.frames.as_ptr().add(t - 1) };
                return phys;
            }
        }
    }

    /// Attempt a lock-free push. Returns false if magazine is full.
    #[inline]
    fn push(&self, phys: u64) -> bool {
        loop {
            let t = self.top.load(Ordering::Acquire);
            if t >= MAGAZINE_SIZE { return false; }
            if self.top.compare_exchange(t, t + 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                unsafe { *(self.frames.as_ptr().add(t) as *mut u64) = phys; }
                return true;
            }
        }
    }

    fn is_empty(&self) -> bool { self.top.load(Ordering::Relaxed) == 0 }
    fn is_full(&self)  -> bool { self.top.load(Ordering::Relaxed) >= MAGAZINE_SIZE }
    fn count(&self)    -> usize { self.top.load(Ordering::Relaxed) }
    fn clear(&self)    { self.top.store(0, Ordering::Release); }
}

static MAGAZINES: [Magazine; MAX_CPUS_PHYS] = {
    [const { Magazine::new() }; MAX_CPUS_PHYS]
};

static TOTAL_FREE: AtomicUsize = AtomicUsize::new(0);
static TOTAL_ALL:  AtomicUsize = AtomicUsize::new(0);

// ── Init ──────────────────────────────────────────────────────────────────

pub fn init(boot_info: &BootInfo) {
    let desc_sz = boot_info.memory_map_descriptor_size as usize;
    let count   = boot_info.memory_map_count as usize;
    let map_ptr = boot_info.memory_map_addr as *const u8;
    let mut total = 0usize;

    let kernel_start = boot_info.kernel_phys_start & !(PAGE_SIZE - 1);
    let kernel_end   = (boot_info.kernel_phys_end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let init_start   = boot_info.init_phys_start & !(PAGE_SIZE - 1);
    let init_end     = (boot_info.init_phys_start + boot_info.init_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
    let qshell_start = boot_info.qshell_phys_start & !(PAGE_SIZE - 1);
    let qshell_end   = (boot_info.qshell_phys_start + boot_info.qshell_size + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

    let is_reserved = |phys: u64| -> bool {
        if phys < 0x10_0000 { return true; }
        if phys >= kernel_start && phys < kernel_end { return true; }
        if boot_info.init_size != 0 && phys >= init_start && phys < init_end { return true; }
        if boot_info.qshell_size != 0 && phys >= qshell_start && phys < qshell_end { return true; }
        false
    };

    {
        let mut b = BUDDY.lock();
        *b = BuddyAlloc::new();
        for i in 0..count {
            let desc = unsafe { &*(map_ptr.add(i * desc_sz) as *const UefiMemoryDescriptor) };
            let usable = matches!(
                desc.mem_type,
                uefi_memory_type::CONVENTIONAL
                | uefi_memory_type::BOOT_SERVICES_CODE
                | uefi_memory_type::BOOT_SERVICES_DATA
                | uefi_memory_type::LOADER_CODE
                | uefi_memory_type::LOADER_DATA
            );
            if usable {
                let desc_start = desc.phys_start.max(0x10_0000);
                let desc_end   = desc.phys_start + desc.num_pages * PAGE_SIZE;
                let mut phys   = desc_start;
                while phys < desc_end {
                    total += 1;
                    if !is_reserved(phys) {
                        b.add_range((phys / PAGE_SIZE) as usize, 1);
                    }
                    phys += PAGE_SIZE;
                }
            }
        }
    }

    TOTAL_ALL.store(total, Ordering::Relaxed);
    TOTAL_FREE.store(BUDDY.lock().free_frames, Ordering::Relaxed);

    crate::klog!("phys: {} MB total, {} MB free",
        total * 4096 / 1048576,
        BUDDY.lock().free_frames * 4096 / 1048576);
}

// ── Public API ────────────────────────────────────────────────────────────

/// Allocate one 4KB frame.
///
/// Fast path: atomic pop from per-CPU magazine (~5 ns on warm cache).
/// Slow path (magazine empty): refill from buddy allocator (~50 ns).
#[inline]
pub fn alloc_frame() -> Option<u64> {
    let cpu = crate::arch::x86_64::smp::current_cpu_id() as usize;
    let cpu = cpu.min(MAX_CPUS_PHYS - 1);

    // Fast path: magazine pop
    let phys = MAGAZINES[cpu].pop();
    if phys != 0 {
        TOTAL_FREE.fetch_sub(1, Ordering::Relaxed);
        return Some(phys);
    }

    // Slow path: refill magazine from buddy
    {
        let mut b = BUDDY.lock();
        let mag = &MAGAZINES[cpu];
        while !mag.is_full() {
            match b.alloc(0) {
                Some(p) => { mag.push(p); }
                None    => break,
            }
        }
    }

    let phys = MAGAZINES[cpu].pop();
    if phys != 0 {
        TOTAL_FREE.fetch_sub(1, Ordering::Relaxed);
        Some(phys)
    } else {
        crate::klog!(
            "phys: alloc_frame failed on cpu {} free={} total={}",
            cpu,
            TOTAL_FREE.load(Ordering::Relaxed),
            TOTAL_ALL.load(Ordering::Relaxed),
        );
        None
    }
}

/// Free one 4KB frame.
///
/// Fast path: atomic push to per-CPU magazine (~5 ns on warm cache).
/// Slow path (magazine full): drain half to buddy allocator.
#[inline]
pub fn free_frame(phys: u64) {
    let cpu = crate::arch::x86_64::smp::current_cpu_id() as usize;
    let cpu = cpu.min(MAX_CPUS_PHYS - 1);

    // Fast path
    if MAGAZINES[cpu].push(phys) {
        TOTAL_FREE.fetch_add(1, Ordering::Relaxed);
        return;
    }

    // Slow path: drain half the magazine to buddy
    {
        let mut b = BUDDY.lock();
        let mag = &MAGAZINES[cpu];
        let drain = mag.count() / 2;
        for _ in 0..drain {
            let p = mag.pop();
            if p != 0 { b.free(p, 0); } else { break; }
        }
    }

    MAGAZINES[cpu].push(phys);
    TOTAL_FREE.fetch_add(1, Ordering::Relaxed);
}

/// Allocate `n` contiguous 4KB frames.
/// Always goes through the buddy allocator (no magazine bypass for contiguous).
#[inline]
pub fn alloc_frames(n: usize) -> Option<u64> {
    if n == 0 { return None; }
    if n == 1 { return alloc_frame(); }

    let order = usize::BITS as usize - n.leading_zeros() as usize
                - if n.is_power_of_two() { 1 } else { 0 };
    let order = order.min(MAX_ORDER);

    let phys = BUDDY.lock().alloc(order)?;
    TOTAL_FREE.fetch_sub(1usize << order, Ordering::Relaxed);
    Some(phys)
}

/// Free `n` contiguous frames.
pub fn free_frames_n(phys: u64, n: usize) {
    if n == 1 { free_frame(phys); return; }
    let order = (usize::BITS as usize - n.leading_zeros() as usize)
                .saturating_sub(1).min(MAX_ORDER);
    BUDDY.lock().free(phys, order);
    TOTAL_FREE.fetch_add(1usize << order, Ordering::Relaxed);
}

pub fn free_frames()  -> usize { TOTAL_FREE.load(Ordering::Relaxed) }
pub fn total_frames() -> usize { TOTAL_ALL.load(Ordering::Relaxed) }

pub fn reserve_range(phys_start: u64, bytes: u64) {
    let first = (phys_start / PAGE_SIZE) as usize;
    let count = ((bytes + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
    BUDDY.lock().mark_range_used(first, count);
}

pub fn usage_percent() -> u32 {
    let total = TOTAL_ALL.load(Ordering::Relaxed);
    let free  = TOTAL_FREE.load(Ordering::Relaxed);
    if total == 0 { return 0; }
    ((total.saturating_sub(free)) * 100 / total) as u32
}
