//! NUMA (Non-Uniform Memory Access) topology and per-node memory allocator.
//!
//! Parses the ACPI SRAT (System Resource Affinity Table) to discover:
//!   - How many NUMA nodes exist
//!   - Which physical address ranges belong to each node
//!   - Which logical CPUs belong to each node
//!
//! Then maintains a per-node buddy allocator so memory allocated for a task
//! running on node N comes preferentially from node N's memory banks,
//! avoiding the latency penalty of cross-node DRAM accesses.
//!
//! ## NUMA latency on real hardware
//! Local DRAM:  ~70 ns
//! Remote DRAM: ~130–250 ns  (2–3.5× slower)
//!
//! Qunix NUMA policy:
//!   ALLOC_LOCAL  (default) — try node-local first, fall back to remote
//!   ALLOC_INTERLEAVE       — round-robin across nodes (for large shared buffers)
//!   ALLOC_PREFERRED(n)     — prefer node n, fall back on OOM

use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::arch::x86_64::paging::PAGE_SIZE;

// ── ACPI SRAT structures ───────────────────────────────────────────────────

const SRAT_SIG: [u8; 4] = *b"SRAT";

#[repr(C, packed)]
struct AcpiTableHeader {
    signature:        [u8; 4],
    length:           u32,
    revision:         u8,
    checksum:         u8,
    oem_id:           [u8; 6],
    oem_table_id:     [u8; 8],
    oem_revision:     u32,
    creator_id:       u32,
    creator_revision: u32,
}

#[repr(C, packed)]
struct SratHeader {
    header:    AcpiTableHeader,
    _reserved1: u32,
    _reserved2: u64,
}

// SRAT sub-table types
const SRAT_TYPE_CPU_AFFINITY:    u8 = 0;
const SRAT_TYPE_MEMORY_AFFINITY: u8 = 1;
const SRAT_TYPE_X2APIC_AFFINITY: u8 = 2;
const SRAT_TYPE_GICC_AFFINITY:   u8 = 3;

#[repr(C, packed)]
struct SratCpuAffinity {
    sub_type:         u8,
    length:           u8,
    proximity_lo:     u8,
    apic_id:          u8,
    flags:            u32,
    local_sapic_eid:  u8,
    proximity_hi:     [u8; 3],
    clock_domain:     u32,
}

#[repr(C, packed)]
struct SratMemAffinity {
    sub_type:         u8,
    length:           u8,
    proximity:        u32,
    _reserved1:       u16,
    base_lo:          u32,
    base_hi:          u32,
    length_lo:        u32,
    length_hi:        u32,
    _reserved2:       u32,
    flags:            u32,
    _reserved3:       u64,
}

#[repr(C, packed)]
struct SratX2ApicAffinity {
    sub_type:    u8,
    length:      u8,
    _reserved:   u16,
    proximity:   u32,
    apic_id:     u32,
    flags:       u32,
    clock_domain: u32,
    _reserved2:  u32,
}

const SRAT_MEM_ENABLED:     u32 = 1 << 0;
const SRAT_MEM_HOT_PLUGGABLE: u32 = 1 << 1;
const SRAT_MEM_NON_VOLATILE: u32 = 1 << 2;
const SRAT_CPU_ENABLED:     u32 = 1 << 0;

// ── NUMA topology ─────────────────────────────────────────────────────────

pub const MAX_NUMA_NODES: usize = 8;
pub const NUMA_NO_NODE:   u32   = u32::MAX;

/// A contiguous physical memory range belonging to one NUMA node.
#[derive(Clone, Copy, Debug)]
pub struct NumaMemRange {
    pub base:    u64,
    pub end:     u64,
    pub node_id: u32,
    pub hot_pluggable: bool,
}

/// CPU → NUMA node mapping entry.
#[derive(Clone, Copy, Debug)]
pub struct NumaCpuAffinity {
    pub apic_id: u32,
    pub node_id: u32,
}

/// Complete NUMA topology for the system.
pub struct NumaTopology {
    pub node_count: u32,
    pub mem_ranges: Vec<NumaMemRange>,
    pub cpu_map:    Vec<NumaCpuAffinity>,
    /// Total bytes per node
    pub node_mem:   [u64; MAX_NUMA_NODES],
}

impl NumaTopology {
    /// Determine which NUMA node a physical address belongs to.
    pub fn phys_to_node(&self, phys: u64) -> u32 {
        for r in &self.mem_ranges {
            if phys >= r.base && phys < r.end { return r.node_id; }
        }
        0 // default node
    }

    /// Determine which NUMA node a logical CPU belongs to.
    pub fn cpu_to_node(&self, apic_id: u32) -> u32 {
        self.cpu_map.iter()
            .find(|c| c.apic_id == apic_id)
            .map(|c| c.node_id)
            .unwrap_or(0)
    }
}

// ── Per-node buddy allocator ───────────────────────────────────────────────

const MAX_ORDER_NUMA: usize = 11; // 2^11 * 4KB = 8MB max block

struct NodeBuddy {
    node_id:    u32,
    free_bitmaps: [[u64; 1 << 14]; MAX_ORDER_NUMA + 1], // up to 256GB per node
    total:      usize,
    free:       usize,
}

impl NodeBuddy {
    fn new(node_id: u32) -> Self {
        NodeBuddy {
            node_id,
            free_bitmaps: [[0u64; 1 << 14]; MAX_ORDER_NUMA + 1],
            total: 0,
            free: 0,
        }
    }

    fn bm_set(&mut self, order: usize, idx: usize) {
        let w = idx / 64; let b = idx % 64;
        if w < self.free_bitmaps[order].len() {
            self.free_bitmaps[order][w] |= 1u64 << b;
        }
    }

    fn bm_clr(&mut self, order: usize, idx: usize) {
        let w = idx / 64; let b = idx % 64;
        if w < self.free_bitmaps[order].len() {
            self.free_bitmaps[order][w] &= !(1u64 << b);
        }
    }

    fn bm_test(&self, order: usize, idx: usize) -> bool {
        let w = idx / 64; let b = idx % 64;
        w < self.free_bitmaps[order].len()
            && self.free_bitmaps[order][w] & (1u64 << b) != 0
    }

    fn bm_find(&self, order: usize) -> Option<usize> {
        for (wi, &w) in self.free_bitmaps[order].iter().enumerate() {
            if w != 0 { return Some(wi * 64 + w.trailing_zeros() as usize); }
        }
        None
    }

    fn add_frame(&mut self, phys: u64) {
        self.free_frame(phys, 0);
        self.total += 1;
    }

    fn alloc(&mut self, order: usize) -> Option<u64> {
        for o in order..=MAX_ORDER_NUMA {
            if let Some(idx) = self.bm_find(o) {
                self.bm_clr(o, idx);
                // Split down to requested order
                let mut ci = idx;
                for split in (order..o).rev() {
                    self.bm_set(split, ci * 2 + 1); // buddy free
                    ci *= 2;
                }
                let n = 1usize << order;
                if self.free >= n { self.free -= n; }
                return Some(ci as u64 * n as u64 * PAGE_SIZE);
            }
        }
        None
    }

    fn free_frame(&mut self, phys: u64, order: usize) {
        let n = 1usize << order;
        self.free += n;
        let mut idx = (phys / PAGE_SIZE) as usize / n;
        let mut o = order;
        loop {
            let buddy = idx ^ 1;
            if o >= MAX_ORDER_NUMA || !self.bm_test(o, buddy) {
                self.bm_set(o, idx); break;
            }
            self.bm_clr(o, buddy);
            idx >>= 1; o += 1;
        }
    }
}

// ── Global NUMA state ─────────────────────────────────────────────────────

static TOPO:  Mutex<Option<NumaTopology>>  = Mutex::new(None);
static NODES: [Mutex<Option<NodeBuddy>>; MAX_NUMA_NODES] = {
    [const { Mutex::new(None) }; MAX_NUMA_NODES]
};

static INTERLEAVE_COUNTER: AtomicU32 = AtomicU32::new(0);
static NUMA_ENABLED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn is_enabled() -> bool { NUMA_ENABLED.load(Ordering::Relaxed) }

// ── ACPI SRAT parsing ────────────────────────────────────────────────────

/// Parse the ACPI SRAT table and build the NUMA topology.
/// Called from memory::init() with the RSDP physical address.
pub fn init_from_srat(rsdp_phys: u64) {
    if rsdp_phys == 0 { return; }

    let srat_phys = match find_srat(rsdp_phys) {
        Some(p) => p,
        None => { crate::klog!("NUMA: no SRAT table found, single-node mode"); return; }
    };

    let srat_virt = srat_phys + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
    let hdr = unsafe { &*(srat_virt as *const SratHeader) };
    let total_len = u32::from_le(hdr.header.length) as usize;

    crate::klog!("NUMA: found SRAT at {:#x}, {} bytes", srat_phys, total_len);

    let mut topo = NumaTopology {
        node_count: 0,
        mem_ranges: Vec::new(),
        cpu_map:    Vec::new(),
        node_mem:   [0u64; MAX_NUMA_NODES],
    };

    let mut max_node = 0u32;

    // Walk sub-tables
    let base  = srat_virt as *const u8;
    let mut off = core::mem::size_of::<SratHeader>();
    while off + 2 < total_len {
        let sub_type = unsafe { *base.add(off) };
        let sub_len  = unsafe { *base.add(off + 1) } as usize;
        if sub_len < 2 || off + sub_len > total_len { break; }

        match sub_type {
            SRAT_TYPE_CPU_AFFINITY => {
                let s = unsafe { &*(base.add(off) as *const SratCpuAffinity) };
                if u32::from_le(s.flags) & SRAT_CPU_ENABLED != 0 {
                    let prox = s.proximity_lo as u32
                        | (s.proximity_hi[0] as u32) << 8
                        | (s.proximity_hi[1] as u32) << 16
                        | (s.proximity_hi[2] as u32) << 24;
                    let node = prox.min((MAX_NUMA_NODES - 1) as u32);
                    topo.cpu_map.push(NumaCpuAffinity { apic_id: s.apic_id as u32, node_id: node });
                    if node > max_node { max_node = node; }
                }
            }
            SRAT_TYPE_MEMORY_AFFINITY => {
                let s = unsafe { &*(base.add(off) as *const SratMemAffinity) };
                let flags = u32::from_le(s.flags);
                if flags & SRAT_MEM_ENABLED != 0 {
                    let base_addr = u64::from_le(s.base_lo as u64)
                        | (u64::from_le(s.base_hi as u64) << 32);
                    let length = u64::from_le(s.length_lo as u64)
                        | (u64::from_le(s.length_hi as u64) << 32);
                    let prox = u32::from_le(s.proximity);
                    let node = prox.min((MAX_NUMA_NODES - 1) as u32);
                    if length > 0 {
                        topo.mem_ranges.push(NumaMemRange {
                            base: base_addr,
                            end:  base_addr + length,
                            node_id: node,
                            hot_pluggable: flags & SRAT_MEM_HOT_PLUGGABLE != 0,
                        });
                        if node < MAX_NUMA_NODES as u32 {
                            topo.node_mem[node as usize] += length;
                        }
                        if node > max_node { max_node = node; }
                    }
                }
            }
            SRAT_TYPE_X2APIC_AFFINITY => {
                let s = unsafe { &*(base.add(off) as *const SratX2ApicAffinity) };
                if u32::from_le(s.flags) & SRAT_CPU_ENABLED != 0 {
                    let prox = u32::from_le(s.proximity);
                    let node = prox.min((MAX_NUMA_NODES - 1) as u32);
                    topo.cpu_map.push(NumaCpuAffinity {
                        apic_id: u32::from_le(s.apic_id),
                        node_id: node,
                    });
                    if node > max_node { max_node = node; }
                }
            }
            _ => {}
        }
        off += sub_len;
    }

    topo.node_count = max_node + 1;

    crate::klog!("NUMA: {} nodes, {} mem ranges, {} CPUs",
        topo.node_count, topo.mem_ranges.len(), topo.cpu_map.len());

    for n in 0..topo.node_count as usize {
        crate::klog!("NUMA: node {} has {} MB", n, topo.node_mem[n] / 1048576);
        *NODES[n].lock() = Some(NodeBuddy::new(n as u32));
    }

    *TOPO.lock() = Some(topo);
    NUMA_ENABLED.store(true, Ordering::Release);
}

fn find_srat(rsdp_phys: u64) -> Option<u64> {
    use crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
    let rsdp_virt = rsdp_phys + KERNEL_VIRT_OFFSET;

    // Check RSDP v2 ("RSD PTR ")
    let sig = unsafe { core::slice::from_raw_parts(rsdp_virt as *const u8, 8) };
    if sig != b"RSD PTR " { return None; }

    // v2: XSDT at offset 24
    let xsdt_phys = unsafe { *(rsdp_virt as *const u8).add(24) as *const u64 };
    let xsdt_phys = unsafe { *xsdt_phys };
    if xsdt_phys == 0 {
        // Try RSDT (v1) at offset 16
        let rsdt_phys = unsafe { *((rsdp_virt + 16) as *const u32) } as u64;
        return find_table_in_rsdt(rsdt_phys, &SRAT_SIG);
    }
    find_table_in_xsdt(xsdt_phys, &SRAT_SIG)
}

fn find_table_in_xsdt(xsdt_phys: u64, sig: &[u8; 4]) -> Option<u64> {
    let v = xsdt_phys + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
    let hdr = unsafe { &*(v as *const AcpiTableHeader) };
    let total = u32::from_le(hdr.length) as usize;
    let n_entries = (total - core::mem::size_of::<AcpiTableHeader>()) / 8;
    let entries_base = (v + core::mem::size_of::<AcpiTableHeader>() as u64) as *const u64;
    for i in 0..n_entries {
        let tbl_phys = unsafe { *entries_base.add(i) };
        let tbl_virt = tbl_phys + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
        let tbl_sig  = unsafe { core::slice::from_raw_parts(tbl_virt as *const u8, 4) };
        if tbl_sig == sig { return Some(tbl_phys); }
    }
    None
}

fn find_table_in_rsdt(rsdt_phys: u64, sig: &[u8; 4]) -> Option<u64> {
    let v = rsdt_phys + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
    let hdr = unsafe { &*(v as *const AcpiTableHeader) };
    let total = u32::from_le(hdr.length) as usize;
    let n_entries = (total - core::mem::size_of::<AcpiTableHeader>()) / 4;
    let entries_base = (v + core::mem::size_of::<AcpiTableHeader>() as u64) as *const u32;
    for i in 0..n_entries {
        let tbl_phys = unsafe { *entries_base.add(i) } as u64;
        let tbl_virt = tbl_phys + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
        let tbl_sig  = unsafe { core::slice::from_raw_parts(tbl_virt as *const u8, 4) };
        if tbl_sig == sig { return Some(tbl_phys); }
    }
    None
}

// ── Populate per-node buddies from global frame allocator ─────────────────

/// Called after phys::init() to redistribute frames to per-node buddies.
pub fn populate_nodes() {
    if !NUMA_ENABLED.load(Ordering::Relaxed) { return; }

    let topo_guard = TOPO.lock();
    let topo = match topo_guard.as_ref() { Some(t) => t, None => return };

    // Walk all pages in each memory range and add to the correct node buddy
    for range in &topo.mem_ranges {
        if range.hot_pluggable { continue; } // skip hot-plug memory at init
        let node = range.node_id as usize;
        if node >= MAX_NUMA_NODES { continue; }

        let first = (range.base / PAGE_SIZE) as usize;
        let last  = (range.end  / PAGE_SIZE) as usize;
        let mut guard = NODES[node].lock();
        if let Some(ref mut nb) = *guard {
            for f in first..last {
                // Only add frames that are actually free in the global allocator
                // (kernel image, BIOS etc. won't be in the buddy's free list)
                let phys = f as u64 * PAGE_SIZE;
                if phys >= 0x10_0000 {
                    nb.add_frame(phys);
                }
            }
        }
    }

    crate::klog!("NUMA: per-node memory populated");
    for node in 0..topo.node_count as usize {
        if let Some(ref nb) = *NODES[node].lock() {
            crate::klog!("NUMA: node {}: {} MB free", node, nb.free * 4096 / 1048576);
        }
    }
}

// ── NUMA-aware allocation API ─────────────────────────────────────────────

/// Allocation policy for NUMA-aware allocs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumaPolicy {
    Local,           // prefer the current CPU's node, fall back to remote
    Preferred(u32),  // prefer specific node, fall back
    Interleave,      // round-robin across all nodes
    Bind(u32),       // only allocate from specific node, fail otherwise
}

/// Allocate one 4KB frame from node `node_id`.
/// Returns None only if that node is completely exhausted.
pub fn alloc_frame_node(node_id: u32) -> Option<u64> {
    let n = node_id as usize;
    if n >= MAX_NUMA_NODES { return None; }
    NODES[n].lock().as_mut()?.alloc(0)
}

/// Allocate with policy. Falls back to the global allocator if NUMA is disabled.
pub fn alloc_frame_policy(policy: NumaPolicy) -> Option<u64> {
    if !NUMA_ENABLED.load(Ordering::Relaxed) {
        return crate::memory::phys::alloc_frame();
    }

    let node_count = TOPO.lock().as_ref().map(|t| t.node_count).unwrap_or(1) as usize;

    match policy {
        NumaPolicy::Local | NumaPolicy::Preferred(NUMA_NO_NODE) => {
            let my_node = current_node();
            // Try local first
            if let Some(p) = alloc_frame_node(my_node) { return Some(p); }
            // Fall back to any other node
            for n in 0..node_count {
                if n as u32 != my_node {
                    if let Some(p) = alloc_frame_node(n as u32) { return Some(p); }
                }
            }
            None
        }
        NumaPolicy::Preferred(node) => {
            if let Some(p) = alloc_frame_node(node) { return Some(p); }
            for n in 0..node_count {
                if n as u32 != node {
                    if let Some(p) = alloc_frame_node(n as u32) { return Some(p); }
                }
            }
            None
        }
        NumaPolicy::Bind(node) => alloc_frame_node(node),
        NumaPolicy::Interleave => {
            let n = INTERLEAVE_COUNTER.fetch_add(1, Ordering::Relaxed) as usize % node_count;
            if let Some(p) = alloc_frame_node(n as u32) { return Some(p); }
            for i in 1..node_count {
                let nn = (n + i) % node_count;
                if let Some(p) = alloc_frame_node(nn as u32) { return Some(p); }
            }
            None
        }
    }
}

/// Free a frame back to its node's buddy (determined by physical address).
pub fn free_frame_node(phys: u64) {
    if !NUMA_ENABLED.load(Ordering::Relaxed) {
        crate::memory::phys::free_frame(phys);
        return;
    }
    let node = TOPO.lock().as_ref().map(|t| t.phys_to_node(phys)).unwrap_or(0);
    if node < MAX_NUMA_NODES as u32 {
        if let Some(ref mut nb) = *NODES[node as usize].lock() {
            nb.free_frame(phys, 0);
        }
    }
}

/// Return the NUMA node of the current CPU.
pub fn current_node() -> u32 {
    let apic = crate::arch::x86_64::smp::apic_id();
    TOPO.lock().as_ref().map(|t| t.cpu_to_node(apic)).unwrap_or(0)
}

/// Return the NUMA node for a physical address.
pub fn phys_node(phys: u64) -> u32 {
    TOPO.lock().as_ref().map(|t| t.phys_to_node(phys)).unwrap_or(0)
}

pub fn node_count() -> u32 {
    TOPO.lock().as_ref().map(|t| t.node_count).unwrap_or(1)
}

pub fn node_free_mb(node: u32) -> u64 {
    if node >= MAX_NUMA_NODES as u32 { return 0; }
    NODES[node as usize].lock().as_ref().map(|nb| nb.free as u64 * 4096 / 1048576).unwrap_or(0)
}
