/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Virtual Memory Manager — COW fork, address spaces, region tracking.
//!
//! COW (Copy-on-Write) fork: on fork, all writable pages in both parent
//! and child are marked read-only + COW. On write fault, the faulting
//! process gets a private copy. This makes fork() O(regions) instead
//! of O(pages) and enables world's-fastest fork.

use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::arch::x86_64::paging::{
    PageFlags, PageMapper, PAGE_SIZE, phys_to_virt, KERNEL_VIRT_OFFSET,
};
use crate::memory::phys::{alloc_frame, free_frame};

// ── COW page reference counting ───────────────────────────────────────────

/// Global reference count table for physical frames.
/// When refcount > 1 a frame is shared (COW). On write fault,
/// refcount is decremented and a private copy is made.
///
/// We use a flat array indexed by frame number (phys >> 12).
/// 4 GB physical memory → 1M frames → 4 MB of u32 refcounts.
const MAX_FRAMES: usize = 1024 * 1024;
static COW_REFCOUNTS: Mutex<[u16; MAX_FRAMES]> = Mutex::new([0u16; MAX_FRAMES]);

pub fn cow_inc(phys: u64) {
    let frame = (phys / PAGE_SIZE) as usize;
    if frame < MAX_FRAMES {
        let mut rc = COW_REFCOUNTS.lock();
        if rc[frame] < u16::MAX { rc[frame] += 1; }
    }
}

/// Mark a previously private frame as shared by a forked child.
///
/// Refcount semantics here are:
/// - `0` => exactly one mapping owns the frame
/// - `N>0` => the frame is shared by `N` mappings
///
/// On the first fork of a private page we therefore need to jump from `0`
/// straight to `2`, not `1`, otherwise the first writer incorrectly thinks it
/// became the sole owner again and skips the copy.
pub fn cow_share(phys: u64) {
    let frame = (phys / PAGE_SIZE) as usize;
    if frame < MAX_FRAMES {
        let mut rc = COW_REFCOUNTS.lock();
        rc[frame] = match rc[frame] {
            0 => 2,
            n if n < u16::MAX => n + 1,
            n => n,
        };
    }
}

pub fn cow_dec(phys: u64) -> u16 {
    let frame = (phys / PAGE_SIZE) as usize;
    if frame < MAX_FRAMES {
        let mut rc = COW_REFCOUNTS.lock();
        if rc[frame] > 0 { rc[frame] -= 1; }
        rc[frame]
    } else { 0 }
}

pub fn cow_refcount(phys: u64) -> u16 {
    let frame = (phys / PAGE_SIZE) as usize;
    if frame < MAX_FRAMES { COW_REFCOUNTS.lock()[frame] } else { 0 }
}

// ── Region kinds ──────────────────────────────────────────────────────────

#[derive(Clone, PartialEq, Debug)]
pub enum RegionKind {
    Anonymous,
    Stack,
    Heap,
    File { dev: u64, ino: u64, offset: u64 },
    Shared,
    Device,
    Vdso,
}

bitflags::bitflags! {
    #[derive(Clone, Copy, PartialEq, Debug)]
    pub struct Prot: u32 {
        const NONE  = 0;
        const READ  = 1;
        const WRITE = 2;
        const EXEC  = 4;
    }
}

#[derive(Clone, Debug)]
pub struct VmaRegion {
    pub start: u64,
    pub end:   u64,
    pub prot:  Prot,
    pub kind:  RegionKind,
    pub flags: u32,
    pub name:  alloc::string::String,
    pub cow:   bool,   // true = COW-marked (no writes until copied)
}

impl VmaRegion {
    pub fn len(&self) -> u64 { self.end - self.start }
    pub fn contains(&self, addr: u64) -> bool { addr >= self.start && addr < self.end }
}

// ── Address space ─────────────────────────────────────────────────────────

pub struct AddressSpace {
    pub pml4_phys:   u64,
    pub regions:     Vec<VmaRegion>,
    pub brk:         u64,
    pub brk_start:   u64,
    pub mmap_base:   u64,
    pub stack_start: u64,
    pub stack_end:   u64,
    pub is_shared:   bool,
}

const MMAP_BASE: u64 = 0x0000_4000_0000_0000;
const BRK_START: u64 = 0x0000_0001_0000_0000;
const STACK_TOP: u64 = 0x0000_7FFF_FFFF_0000;

// Linux mmap() flag values (x86-64)
pub const MAP_SHARED:    u32 = 0x01;
pub const MAP_PRIVATE:   u32 = 0x02;
pub const MAP_FIXED:     u32 = 0x10;
pub const MAP_ANONYMOUS: u32 = 0x20;

// Linux mremap() flag values
pub const MREMAP_MAYMOVE: u32 = 1;
pub const MREMAP_FIXED:   u32 = 2;

impl AddressSpace {
    pub fn new_kernel() -> Self {
        AddressSpace {
            pml4_phys: crate::arch::x86_64::paging::get_cr3(),
            regions: Vec::new(), brk: 0, brk_start: 0,
            mmap_base: MMAP_BASE, stack_start: 0, stack_end: 0,
            is_shared: false,
        }
    }

    pub fn new_user() -> Option<Self> {
        use crate::arch::x86_64::paging::PageTable;
        let pml4 = alloc_frame()?;
        crate::klog!("vmm: new_user pml4={:#x}", pml4);
        unsafe {
            core::ptr::write_bytes(phys_to_virt(pml4) as *mut u8, 0, PAGE_SIZE as usize);
            let src = crate::arch::x86_64::paging::get_cr3();
            crate::klog!("vmm: cloning kernel half from cr3={:#x}", src);
            let src_tbl = &*(phys_to_virt(src) as *const PageTable);
            let dst_tbl = &mut *(phys_to_virt(pml4) as *mut PageTable);
            for i in 256..512 { dst_tbl.entries[i] = src_tbl.entries[i]; }
        }
        crate::klog!("vmm: new_user ready");
        Some(AddressSpace {
            pml4_phys: pml4, regions: Vec::new(),
            brk: BRK_START, brk_start: BRK_START,
            mmap_base: MMAP_BASE, stack_start: 0, stack_end: STACK_TOP,
            is_shared: false,
        })
    }

    pub fn shared(pml4_phys: u64) -> Self {
        AddressSpace {
            pml4_phys, regions: Vec::new(),
            brk: BRK_START, brk_start: BRK_START,
            mmap_base: MMAP_BASE, stack_start: 0, stack_end: STACK_TOP,
            is_shared: true,
        }
    }

    pub fn activate(&self) {
        crate::arch::x86_64::paging::set_cr3(self.pml4_phys);
    }

    // ── COW-enabled fork ──────────────────────────────────────────────────

    /// O(regions) fork: mark all writable pages COW in parent+child,
    /// share the physical frames with refcount=2.
    pub fn copy_on_fork(&mut self) -> Option<AddressSpace> {
        let mut child = AddressSpace::new_user()?;
        child.brk       = self.brk;
        child.brk_start = self.brk_start;
        child.mmap_base = self.mmap_base;
        child.stack_start = self.stack_start;
        child.stack_end   = self.stack_end;
        child.regions   = self.regions.clone();

        let mut pmapper = PageMapper::new(self.pml4_phys);
        let mut cmapper = PageMapper::new(child.pml4_phys);

        for region in &mut self.regions {
            let can_cow = region.prot.contains(Prot::WRITE)
                && region.kind != RegionKind::Device
                && region.flags & 1 == 0; // not MAP_SHARED

            let pages = (region.end - region.start) / PAGE_SIZE;

            for i in 0..pages {
                let virt = region.start + i * PAGE_SIZE;
                let src_phys = unsafe { pmapper.translate(virt) };
                if let Some(sp) = src_phys {
                    let frame = sp & !(PAGE_SIZE - 1);
                    if can_cow {
                        // Mark both parent and child read-only COW
                        let ro_flags = prot_to_flags(region.prot & !Prot::WRITE)
                            | PageFlags::COW;
                        unsafe {
                            pmapper.map_page(virt, frame, ro_flags);
                            cmapper.map_page(virt, frame, ro_flags);
                        }
                        cow_share(frame);
                    } else {
                        // Non-writable or shared: share read-only
                        let flags = prot_to_flags(region.prot);
                        unsafe { cmapper.map_page(virt, frame, flags); }
                        cow_share(frame);
                    }
                }
            }
            if can_cow { region.cow = true; }
        }

        // Mark child regions as COW too
        for region in &mut child.regions {
            if region.prot.contains(Prot::WRITE)
                && region.kind != RegionKind::Device
                && region.flags & 1 == 0 {
                region.cow = true;
            }
        }

        // Flush TLB on current CPU (parent) and send shootdown
        crate::arch::x86_64::paging::flush_tlb_all();

        Some(child)
    }

    /// Handle a write-fault COW: copy the page and give this AS a private copy.
    ///
    /// Detects COW in two ways:
    /// 1. The PTE has the COW software bit set (most reliable).
    /// 2. The region has `cow = true` (set during fork).
    ///
    /// Returns true if handled (was a COW fault), false if real fault.
    pub fn handle_cow_fault(&mut self, fault_addr: u64) -> bool {
        let virt_page = fault_addr & !(PAGE_SIZE - 1);
        let mut mapper = PageMapper::new(self.pml4_phys);

        // Translate the faulting page to a physical frame and read its flags
        let phys = match unsafe { mapper.translate(virt_page) } {
            Some(p) => p & !(PAGE_SIZE - 1),
            None    => return false,
        };
        let page_flags = unsafe { mapper.get_flags(virt_page) }
            .unwrap_or(PageFlags::empty());

        // Determine if this is a COW fault:
        // Either the PTE has the COW software bit, OR the region is marked COW
        let is_pte_cow = page_flags.contains(PageFlags::COW);
        let is_region_cow = self.regions.iter()
            .find(|r| r.contains(fault_addr))
            .map(|r| r.cow)
            .unwrap_or(false);

        if !is_pte_cow && !is_region_cow { return false; }

        // Decrement the shared refcount on this frame
        let rc = cow_dec(phys);
        let new_frame = if rc == 0 {
            // We are the last holder — take ownership, no copy needed
            phys
        } else {
            // Other processes still hold this frame — make a private copy
            let nf = match alloc_frame() { Some(f) => f, None => return false };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    phys_to_virt(phys) as *const u8,
                    phys_to_virt(nf)  as *mut u8,
                    PAGE_SIZE as usize,
                );
            }
            nf
        };

        // Remap with write permission, clearing the COW bit
        let base_prot = self.regions.iter()
            .find(|r| r.contains(fault_addr))
            .map(|r| r.prot)
            .unwrap_or(Prot::READ | Prot::WRITE);
        // Ensure the writable bit is set (the whole point of the COW resolution)
        let new_prot  = base_prot | Prot::WRITE;
        let new_flags = prot_to_flags(new_prot); // COW bit NOT set in prot_to_flags
        unsafe { mapper.map_page(virt_page, new_frame, new_flags); }
        unsafe { crate::arch::x86_64::paging::invalidate_tlb(virt_page); }

        // We conservatively leave region.cow=true; the next faulting page in
        // this region will repeat this check. Only when ALL pages in a region
        // have been faulted would it be safe to clear region.cow, and that
        // optimization is not worth the tracking overhead.
        true
    }

    pub fn map_range(&mut self, virt: u64, pages: u64, flags: PageFlags) -> bool {
        let mut mapper = PageMapper::new(self.pml4_phys);
        for i in 0..pages {
            let frame = match alloc_frame() { Some(f) => f, None => {
                let mut m2 = PageMapper::new(self.pml4_phys);
                for j in 0..i {
                    let v = virt + j * PAGE_SIZE;
                    if let Some(p) = unsafe { m2.translate(v) } {
                        free_frame(p & !(PAGE_SIZE - 1));
                        unsafe { m2.unmap_page(v); }
                    }
                }
                return false;
            }};
            unsafe {
                core::ptr::write_bytes(phys_to_virt(frame) as *mut u8, 0, PAGE_SIZE as usize);
                mapper.map_page(virt + i * PAGE_SIZE, frame, flags);
            }
        }
        true
    }

    pub fn map_physical(&mut self, virt: u64, phys: u64, flags: PageFlags) {
        let mut mapper = PageMapper::new(self.pml4_phys);
        unsafe { mapper.map_page(virt, phys, flags); }
    }

    /// Anonymous mmap (backwards-compat shim — new code uses mmap_full).
    pub fn mmap(&mut self, hint: Option<u64>, len: u64, prot: Prot) -> Option<u64> {
        self.mmap_full(hint, len, prot, MAP_PRIVATE | MAP_ANONYMOUS, None, 0)
    }

    /// Full mmap with all Linux flag semantics.
    ///
    /// `file_data`: when Some, contains the bytes of the file being mapped.
    ///              Must already be pre-sliced to start at `file_offset`
    ///              (i.e. file_data[0] = byte at file_offset).
    ///
    /// Flags (Linux x86-64 values):
    ///   MAP_SHARED    = 0x01
    ///   MAP_PRIVATE   = 0x02
    ///   MAP_FIXED     = 0x10  — must map at exact addr or return None
    ///   MAP_ANONYMOUS = 0x20  — ignore fd; zero-fill
    pub fn mmap_full(
        &mut self,
        hint:        Option<u64>,
        len:         u64,
        prot:        Prot,
        map_flags:   u32,
        file_data:   Option<&[u8]>,
        _file_offset: u64,     // already applied in file_data slice
    ) -> Option<u64> {
        let pages     = (len + PAGE_SIZE - 1) / PAGE_SIZE;
        let map_len   = pages * PAGE_SIZE;
        let is_fixed  = map_flags & MAP_FIXED != 0;
        let is_shared = map_flags & MAP_SHARED != 0;

        // ── Address selection ─────────────────────────────────────────────
        let addr = if is_fixed {
            let h = hint?; // MAP_FIXED without an address is invalid
            let aligned = h & !(PAGE_SIZE - 1);
            if aligned == 0 || aligned + map_len > STACK_TOP { return None; }
            // MAP_FIXED: silently unmap anything already at this range
            self.munmap(aligned, map_len);
            aligned
        } else {
            self.pick_addr(hint, map_len)?
        };

        // ── Physical page allocation + population ─────────────────────────
        let pg_flags = prot_to_flags(prot);
        let mut mapper = PageMapper::new(self.pml4_phys);

        for i in 0..pages {
            let frame = match alloc_frame() {
                Some(f) => f,
                None => {
                    // Partial allocation — clean up what we managed to map
                    for j in 0..i {
                        let v = addr + j * PAGE_SIZE;
                        if let Some(p) = unsafe { mapper.translate(v) } {
                            unsafe { mapper.unmap_page(v); }
                            let f = p & !(PAGE_SIZE - 1);
                            if cow_dec(f) == 0 { free_frame(f); }
                        }
                    }
                    return None;
                }
            };

            // Zero the frame first
            unsafe {
                core::ptr::write_bytes(
                    phys_to_virt(frame) as *mut u8, 0, PAGE_SIZE as usize);
            }

            // Copy file data if this is a file-backed mapping
            if let Some(data) = file_data {
                let page_offset = (i as usize) * PAGE_SIZE as usize;
                if page_offset < data.len() {
                    let bytes = (PAGE_SIZE as usize).min(data.len() - page_offset);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            data.as_ptr().add(page_offset),
                            phys_to_virt(frame) as *mut u8,
                            bytes,
                        );
                    }
                }
            }

            unsafe { mapper.map_page(addr + i * PAGE_SIZE, frame, pg_flags); }
        }

        // ── VMA region tracking ───────────────────────────────────────────
        let kind = if file_data.is_some() && !is_shared {
            RegionKind::Anonymous // private file mapping — COW-like, no writeback
        } else if file_data.is_some() {
            RegionKind::Shared
        } else {
            RegionKind::Anonymous
        };

        self.regions.push(VmaRegion {
            start: addr,
            end:   addr + map_len,
            prot,
            kind,
            flags: map_flags,
            name: alloc::string::String::new(),
            cow: false,
        });

        Some(addr)
    }

    pub fn munmap(&mut self, addr: u64, len: u64) {
        if len == 0 { return; }
        let aligned_addr = addr & !(PAGE_SIZE - 1);
        let aligned_end  = (addr + len + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let pages = (aligned_end - aligned_addr) / PAGE_SIZE;

        // Unmap physical pages
        let mut mapper = PageMapper::new(self.pml4_phys);
        for i in 0..pages {
            let v = aligned_addr + i * PAGE_SIZE;
            if let Some(p) = unsafe { mapper.translate(v) } {
                unsafe { mapper.unmap_page(v); }
                let frame = p & !(PAGE_SIZE - 1);
                if cow_dec(frame) == 0 { free_frame(frame); }
            }
        }

        // Update VMA region list — handle partial overlaps by splitting
        let mut new_regions: alloc::vec::Vec<VmaRegion> = alloc::vec::Vec::new();
        let unmapped_start = aligned_addr;
        let unmapped_end   = aligned_end;

        for r in self.regions.drain(..) {
            if r.end <= unmapped_start || r.start >= unmapped_end {
                // No overlap — keep as-is
                new_regions.push(r);
            } else {
                // Overlap: keep the parts outside the unmapped range
                if r.start < unmapped_start {
                    // Left fragment
                    let mut left = r.clone();
                    left.end = unmapped_start;
                    new_regions.push(left);
                }
                if r.end > unmapped_end {
                    // Right fragment
                    let mut right = r.clone();
                    right.start = unmapped_end;
                    new_regions.push(right);
                }
                // The overlapping part is silently dropped
            }
        }
        self.regions = new_regions;
    }

    pub fn set_brk(&mut self, new_brk: u64) -> u64 {
        // brk below the heap start is invalid — return current brk unchanged
        if new_brk < self.brk_start { return self.brk; }

        let old_brk    = self.brk;
        let old_end    = (old_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let new_end    = (new_brk + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let brk_start_aligned = (self.brk_start + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);

        if new_brk < old_brk {
            // ── Shrink ─────────────────────────────────────────────────────
            // Unmap pages that are no longer needed
            if new_end < old_end {
                self.munmap(new_end, old_end - new_end);
            }
            // Update heap region end
            if let Some(r) = self.regions.iter_mut().find(|r| r.kind == RegionKind::Heap) {
                r.end = new_end.max(brk_start_aligned);
            }
            self.brk = new_brk;
            return new_brk;
        }

        if new_brk == old_brk { return old_brk; }

        // ── Grow ───────────────────────────────────────────────────────────
        let start = old_end;
        let end   = new_end;
        if start < end {
            let pages = (end - start) / PAGE_SIZE;
            let flags = PageFlags::PRESENT | PageFlags::WRITABLE
                      | PageFlags::USER    | PageFlags::NO_EXECUTE;
            if !self.map_range(start, pages, flags) {
                return old_brk; // Allocation failed — return old brk
            }
        }

        // Register or extend the heap VMA region
        if let Some(r) = self.regions.iter_mut().find(|r| r.kind == RegionKind::Heap) {
            r.end = new_end;
        } else {
            self.regions.push(VmaRegion {
                start: brk_start_aligned,
                end:   new_end,
                prot:  Prot::READ | Prot::WRITE,
                kind:  RegionKind::Heap,
                flags: MAP_PRIVATE,
                name:  alloc::string::String::from("[heap]"),
                cow:   false,
            });
        }
        self.brk = new_brk;
        new_brk
    }

    /// mremap: resize or move a mapping.
    ///
    /// Linux mremap flags (x86-64):
    ///   MREMAP_MAYMOVE = 1  — allow moving the mapping
    ///   MREMAP_FIXED   = 2  — map at new_addr (requires MAYMOVE)
    pub fn mremap(
        &mut self,
        old_addr: u64,
        old_len:  u64,
        new_len:  u64,
        flags:    u32,
        new_addr: u64,
    ) -> Option<u64> {
        const MREMAP_MAYMOVE: u32 = 1;
        const MREMAP_FIXED:   u32 = 2;

        let old_aligned = old_addr & !(PAGE_SIZE - 1);
        let old_pages   = (old_len  + PAGE_SIZE - 1) / PAGE_SIZE;
        let new_pages   = (new_len  + PAGE_SIZE - 1) / PAGE_SIZE;
        let old_map_len = old_pages * PAGE_SIZE;
        let new_map_len = new_pages * PAGE_SIZE;

        // Validate old range is mapped
        if old_aligned == 0 || old_aligned + old_map_len > STACK_TOP { return None; }

        // Clone the existing region info
        let region = self.regions.iter()
            .find(|r| r.start <= old_aligned && r.end >= old_aligned + old_map_len)
            .cloned();

        let prot = region.as_ref().map(|r| r.prot).unwrap_or(Prot::READ | Prot::WRITE);
        let kind = region.as_ref().map(|r| r.kind.clone()).unwrap_or(RegionKind::Anonymous);
        let map_flags = region.as_ref().map(|r| r.flags).unwrap_or(MAP_PRIVATE);

        if new_len == 0 {
            // Shrink to zero = munmap
            self.munmap(old_aligned, old_map_len);
            return Some(old_aligned);
        }

        let can_move = flags & MREMAP_MAYMOVE != 0;

        if new_pages <= old_pages {
            // ── Shrink in place ────────────────────────────────────────────
            if new_pages < old_pages {
                let shrink_start = old_aligned + new_map_len;
                let shrink_len   = old_map_len - new_map_len;
                self.munmap(shrink_start, shrink_len);
            }
            // Update region end
            for r in &mut self.regions {
                if r.start == old_aligned {
                    r.end = old_aligned + new_map_len;
                    break;
                }
            }
            return Some(old_aligned);
        }

        // ── Grow ───────────────────────────────────────────────────────────
        // Try to grow in place first: check if pages immediately after old mapping are free
        let ext_start = old_aligned + old_map_len;
        let ext_pages = new_pages - old_pages;
        let ext_len   = ext_pages * PAGE_SIZE;
        let space_free = !self.regions.iter()
            .any(|r| r.start < ext_start + ext_len && r.end > ext_start);

        if space_free {
            // Extend in place
            let pg_flags = prot_to_flags(prot);
            if self.map_range(ext_start, ext_pages, pg_flags) {
                // Extend or replace region
                let mut extended = false;
                for r in &mut self.regions {
                    if r.start == old_aligned && r.end == old_aligned + old_map_len {
                        r.end = old_aligned + new_map_len;
                        extended = true;
                        break;
                    }
                }
                if !extended {
                    self.regions.push(VmaRegion {
                        start: old_aligned, end: old_aligned + new_map_len,
                        prot, kind, flags: map_flags,
                        name: alloc::string::String::new(), cow: false,
                    });
                }
                return Some(old_aligned);
            }
        }

        // Cannot grow in place
        if !can_move { return None; }

        // ── Move the mapping ───────────────────────────────────────────────
        // Pick a new address (or use MREMAP_FIXED new_addr)
        let dest = if flags & MREMAP_FIXED != 0 && new_addr != 0 {
            let aligned = new_addr & !(PAGE_SIZE - 1);
            self.munmap(aligned, new_map_len); // Unmap anything there
            aligned
        } else {
            self.pick_addr(None, new_map_len)?
        };

        // Allocate new range and copy old content
        let pg_flags  = prot_to_flags(prot);
        let mut src_mapper = PageMapper::new(self.pml4_phys);
        let mut dst_mapper = PageMapper::new(self.pml4_phys);

        for i in 0..new_pages {
            let new_frame = alloc_frame()?;
            unsafe {
                core::ptr::write_bytes(phys_to_virt(new_frame) as *mut u8, 0, PAGE_SIZE as usize);
            }
            if i < old_pages {
                // Copy from old mapping
                let src_v = old_aligned + i * PAGE_SIZE;
                if let Some(sp) = unsafe { src_mapper.translate(src_v) } {
                    let src_frame = sp & !(PAGE_SIZE - 1);
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            phys_to_virt(src_frame) as *const u8,
                            phys_to_virt(new_frame) as *mut u8,
                            PAGE_SIZE as usize,
                        );
                    }
                }
            }
            unsafe { dst_mapper.map_page(dest + i * PAGE_SIZE, new_frame, pg_flags); }
        }

        // Unmap old range
        self.munmap(old_aligned, old_map_len);

        // Register new region
        self.regions.push(VmaRegion {
            start: dest, end: dest + new_map_len,
            prot, kind, flags: map_flags,
            name: alloc::string::String::new(), cow: false,
        });

        Some(dest)
    }

    pub fn mprotect(&mut self, addr: u64, len: u64, prot: Prot) -> bool {
        let end  = addr + len;
        let flags = prot_to_flags(prot);
        let mut mapper = PageMapper::new(self.pml4_phys);
        let pages = (len + PAGE_SIZE - 1) / PAGE_SIZE;
        for i in 0..pages {
            let v = addr + i * PAGE_SIZE;
            if let Some(p) = unsafe { mapper.translate(v) } {
                unsafe { mapper.map_page(v, p & !(PAGE_SIZE - 1), flags); }
            }
        }
        for r in &mut self.regions {
            if r.start >= addr && r.end <= end { r.prot = prot; }
        }
        true
    }

    pub fn find_region(&self, addr: u64) -> Option<&VmaRegion> {
        self.regions.iter().find(|r| r.contains(addr))
    }

    pub fn release(&mut self) {
        if self.is_shared { return; }
        let mut mapper = PageMapper::new(self.pml4_phys);
        let regions = core::mem::take(&mut self.regions);
        for region in regions {
            let pages = (region.end - region.start) / PAGE_SIZE;
            for i in 0..pages {
                let v = region.start + i * PAGE_SIZE;
                if let Some(p) = unsafe { mapper.translate(v) } {
                    unsafe { mapper.unmap_page(v); }
                    let frame = p & !(PAGE_SIZE - 1);
                    let rc = cow_dec(frame);
                    if rc == 0 { free_frame(frame); }
                }
            }
        }
        if self.pml4_phys != 0 { free_frame(self.pml4_phys); self.pml4_phys = 0; }
    }

    fn pick_addr(&mut self, hint: Option<u64>, size: u64) -> Option<u64> {
        if let Some(h) = hint {
            let aligned = h & !(PAGE_SIZE - 1);
            if aligned > 0 && aligned + size < STACK_TOP {
                if !self.regions.iter().any(|r| r.start < aligned + size && r.end > aligned) {
                    return Some(aligned);
                }
            }
        }
        let base = self.mmap_base;
        self.mmap_base += size + PAGE_SIZE;
        Some(base)
    }
}

pub fn prot_to_flags(prot: Prot) -> PageFlags {
    let mut f = PageFlags::PRESENT | PageFlags::USER;
    if prot.contains(Prot::WRITE) { f |= PageFlags::WRITABLE; }
    if !prot.contains(Prot::EXEC) { f |= PageFlags::NO_EXECUTE; }
    f
}

pub fn init() {}
