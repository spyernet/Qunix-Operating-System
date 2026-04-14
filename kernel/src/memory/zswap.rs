/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! zswap / zram — in-memory compressed page cache.
//!
//! ## What this does
//! When the system runs low on free physical frames, instead of triggering
//! full OOM or writing pages to disk (expensive), zswap compresses evicted
//! pages and stores them in a dedicated compressed memory pool. Frequently
//! re-accessed pages are decompressed back on demand.
//!
//! ## Architecture
//!
//!   ┌──────────────────────────────────────────────────────┐
//!   │ VMM page eviction path                               │
//!   │  → calls zswap_store(pid, vaddr, page_data)          │
//!   │  ← returns a compressed page handle (u64)            │
//!   └──────────────────────────────────────────────────────┘
//!             │
//!             ▼
//!   ┌──────────────────────────────────────────────────────┐
//!   │ Compressed pool (CompressedPool)                     │
//!   │  Slab of 4 KB slots, each holding ≤4 KB of LZ4 data  │
//!   │  High watermark = ZSWAP_MAX_PAGES (default 20% RAM)  │
//!   │  When full: writeback to disk or OOM                 │
//!   └──────────────────────────────────────────────────────┘
//!
//! ## Compression: LZ4 (simplified streaming variant)
//! LZ4 is the right choice here:
//!   - Decompression: ~3 GB/s (a 4KB page decompresses in ~1.3 μs)
//!   - Compression:   ~400 MB/s (encoding a 4KB page ≈ 10 μs)
//!   - Ratio:         1.5–3× for typical process memory pages
//!   - No allocation needed: both compress/decompress work on fixed buffers
//!
//! We implement the LZ4 block format (not the frame format):
//!   Token byte: high 4 bits = literal_len, low 4 bits = match_len
//!   Literals: raw bytes
//!   Offset:   16-bit little-endian back-reference distance
//!   Match:    copy [match_len+4] bytes from [offset] back in output
//!
//! ## zram (RAM disk)
//! A block device backed by compressed pages. Created as /dev/zram0.
//! Userspace can use it as swap:
//!   mkswap /dev/zram0 && swapon /dev/zram0
//! Each 512-byte sector group is individually compressed.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

// ── Constants ────────────────────────────────────────────────────────────

const ZSWAP_MAX_PERCENT:    usize = 20;  // max % of RAM usable by zswap
const ZSWAP_BUCKET_SIZE:    usize = 4096;
const ZSWAP_HEADER_SIZE:    usize = 8;   // compressed_len (u32) + orig_crc (u32)
const ZSWAP_MAX_COMPRESSED: usize = ZSWAP_BUCKET_SIZE - ZSWAP_HEADER_SIZE;

const ZRAM_SECTOR_SIZE:     usize = 512;
const ZRAM_MAX_SECTORS:     u64   = 1 << 20; // 512 MB max zram device

// ── Compressed page entry ─────────────────────────────────────────────────

#[derive(Clone)]
struct ZswapEntry {
    pid:           u32,
    vaddr:         u64,
    compressed:    Vec<u8>,
    orig_checksum: u32,
}

// ── Compressed pool ────────────────────────────────────────────────────────

struct CompressedPool {
    entries:       BTreeMap<u64, ZswapEntry>, // handle → entry
    next_handle:   u64,
    max_entries:   usize,
    total_orig:    u64,   // bytes of original pages stored
    total_comp:    u64,   // bytes of compressed data used
}

impl CompressedPool {
    fn new(max_entries: usize) -> Self {
        CompressedPool {
            entries: BTreeMap::new(),
            next_handle: 1,
            max_entries,
            total_orig: 0,
            total_comp: 0,
        }
    }

    fn is_full(&self) -> bool { self.entries.len() >= self.max_entries }

    fn store(&mut self, pid: u32, vaddr: u64, data: &[u8]) -> Option<u64> {
        if self.is_full() { return None; }
        let crc = crc32_simple(data);
        let mut compressed = alloc::vec![0u8; data.len() + 64]; // slight over-alloc
        let comp_len = lz4_compress(data, &mut compressed)?;
        compressed.truncate(comp_len);

        let handle = self.next_handle;
        self.next_handle += 1;
        self.total_orig += data.len() as u64;
        self.total_comp += comp_len as u64;
        self.entries.insert(handle, ZswapEntry { pid, vaddr, compressed, orig_checksum: crc });
        Some(handle)
    }

    fn load(&mut self, handle: u64, out: &mut [u8]) -> bool {
        let entry = match self.entries.get(&handle) { Some(e) => e, None => return false };
        let ok = lz4_decompress(&entry.compressed, out);
        if ok {
            let crc = crc32_simple(out);
            ok && crc == entry.orig_checksum
        } else { false }
    }

    fn remove(&mut self, handle: u64) -> bool {
        if let Some(e) = self.entries.remove(&handle) {
            self.total_comp -= e.compressed.len() as u64;
            self.total_orig -= 4096;
            true
        } else { false }
    }

    fn evict_oldest(&mut self) -> Option<u64> {
        let handle = *self.entries.keys().next()?;
        self.remove(handle);
        Some(handle)
    }

    fn compression_ratio(&self) -> f32 {
        if self.total_comp == 0 { return 1.0; }
        self.total_orig as f32 / self.total_comp as f32
    }
}

static POOL: Mutex<Option<CompressedPool>> = Mutex::new(None);
static STORED_PAGES:    AtomicUsize = AtomicUsize::new(0);
static LOADED_PAGES:    AtomicUsize = AtomicUsize::new(0);
static EVICTED_PAGES:   AtomicUsize = AtomicUsize::new(0);

pub fn init() {
    let total_frames = crate::memory::phys::total_frames();
    let max_entries  = (total_frames * ZSWAP_MAX_PERCENT / 100).max(256);
    *POOL.lock() = Some(CompressedPool::new(max_entries));
    crate::klog!("zswap: initialized, max {} pages ({} MB compressed pool)",
        max_entries, max_entries * 4096 / 1048576 / 2); // assuming 2:1 ratio
}

pub fn is_enabled() -> bool { POOL.lock().is_some() }

// ── Public zswap API ─────────────────────────────────────────────────────

/// Compress and store one 4KB page. Returns an opaque handle.
/// Returns None if pool is full.
pub fn store_page(pid: u32, vaddr: u64, data: &[u8; 4096]) -> Option<u64> {
    debug_assert_eq!(data.len(), 4096);
    let handle = POOL.lock().as_mut()?.store(pid, vaddr, data)?;
    STORED_PAGES.fetch_add(1, Ordering::Relaxed);
    Some(handle)
}

/// Decompress and retrieve a stored page. Removes it from pool.
/// Returns false if handle invalid or data corrupted.
pub fn load_page(handle: u64, out: &mut [u8; 4096]) -> bool {
    let ok = POOL.lock().as_mut().map(|p| p.load(handle, out)).unwrap_or(false);
    if ok {
        // Remove from pool after successful load (it will be paged back in)
        POOL.lock().as_mut().map(|p| p.remove(handle));
        LOADED_PAGES.fetch_add(1, Ordering::Relaxed);
    }
    ok
}

/// Force-evict the oldest entry (called when pool is full and we must make room).
pub fn evict_one() -> bool {
    let evicted = POOL.lock().as_mut().and_then(|p| p.evict_oldest()).is_some();
    if evicted { EVICTED_PAGES.fetch_add(1, Ordering::Relaxed); }
    evicted
}

pub fn stats() -> ZswapStats {
    let (stored, loaded, evicted) = (
        STORED_PAGES.load(Ordering::Relaxed),
        LOADED_PAGES.load(Ordering::Relaxed),
        EVICTED_PAGES.load(Ordering::Relaxed),
    );
    let (ratio, pool_used) = POOL.lock().as_ref().map(|p| (
        p.compression_ratio(),
        p.entries.len(),
    )).unwrap_or((1.0, 0));
    ZswapStats { stored, loaded, evicted, pool_used, compression_ratio: ratio }
}

#[derive(Clone, Copy, Debug)]
pub struct ZswapStats {
    pub stored:             usize,
    pub loaded:             usize,
    pub evicted:            usize,
    pub pool_used:          usize,
    pub compression_ratio:  f32,
}

// ── LZ4 block compressor/decompressor ────────────────────────────────────
//
// LZ4 block format:
//   Token (1 byte): [lit_len:4][mat_len:4]
//   If lit_len  == 15: read more bytes until <255, add to len
//   Literals (lit_len bytes): raw data to copy
//   Offset (2 bytes LE): back-reference distance (1-based)
//   If mat_len == 15: read more bytes until <255, add to len
//   Match: copy (mat_len+4) bytes from output[-offset]
//
// Last sequence has no offset/match (EOF after literals).

const LZ4_MIN_MATCH:        usize = 4;
const LZ4_HASH_BITS:        usize = 16;
const LZ4_HASH_SIZE:        usize = 1 << LZ4_HASH_BITS;
const LZ4_HASH_MASK:        usize = LZ4_HASH_SIZE - 1;
const LZ4_MAX_DISTANCE:     usize = 65535;
const LZ4_SKIP_TRIGGER:     usize = 6;

#[inline(always)]
fn lz4_hash(val: u32) -> usize {
    // Multiply-based hash for 4-byte sequences
    ((val as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15_u64) >> (64 - LZ4_HASH_BITS)) as usize
}

fn write_varint(out: &mut Vec<u8>, mut n: usize) {
    while n >= 255 { out.push(255); n -= 255; }
    out.push(n as u8);
}

/// LZ4 block compress. Returns compressed size or None on failure (incompressible).
pub fn lz4_compress(src: &[u8], dst: &mut [u8]) -> Option<usize> {
    let n = src.len();
    if n < LZ4_MIN_MATCH { dst[..n].copy_from_slice(src); return Some(n); }

    let mut hash_table = [0u16; LZ4_HASH_SIZE];
    let mut out = Vec::with_capacity(n);
    let mut ip = 0usize;    // input position
    let mut anchor = 0usize;
    let mut step = 1usize;
    let mut skip = LZ4_SKIP_TRIGGER;

    macro_rules! read4 {
        ($p:expr) => {
            if $p + 4 <= n {
                u32::from_le_bytes(src[$p..$p+4].try_into().unwrap_or([0;4]))
            } else { 0 }
        }
    }

    while ip + LZ4_MIN_MATCH + 1 < n {
        let seq  = read4!(ip);
        let h    = lz4_hash(seq);
        let ref_pos = hash_table[h] as usize;
        hash_table[h] = ip as u16;

        let dist = ip.wrapping_sub(ref_pos);
        if dist < LZ4_MAX_DISTANCE && dist > 0
            && ref_pos + LZ4_MIN_MATCH <= n
            && read4!(ref_pos) == seq
        {
            // We have a match. Find its length.
            let lit_len = ip - anchor;
            let mut mat_len = LZ4_MIN_MATCH;
            while ip + mat_len < n && ref_pos + mat_len < ip
                  && src[ip + mat_len] == src[ref_pos + mat_len]
            {
                mat_len += 1;
            }
            let enc_mat = mat_len - LZ4_MIN_MATCH;

            // Token
            let tok_lit = lit_len.min(15) as u8;
            let tok_mat = enc_mat.min(15) as u8;
            out.push((tok_lit << 4) | tok_mat);

            // Extra literal length bytes
            if lit_len >= 15 { write_varint(&mut out, lit_len - 15); }
            // Literal bytes
            out.extend_from_slice(&src[anchor..anchor + lit_len]);
            // Offset
            out.push(dist as u8); out.push((dist >> 8) as u8);
            // Extra match length bytes
            if enc_mat >= 15 { write_varint(&mut out, enc_mat - 15); }

            ip += mat_len;
            anchor = ip;
            step = 1;
            skip = LZ4_SKIP_TRIGGER;
        } else {
            ip += step;
            skip -= 1;
            if skip == 0 { step += 1; skip = LZ4_SKIP_TRIGGER; }
        }
    }

    // Final literal run (no match)
    let lit_len = n - anchor;
    let tok = (lit_len.min(15) as u8) << 4;
    out.push(tok);
    if lit_len >= 15 { write_varint(&mut out, lit_len - 15); }
    out.extend_from_slice(&src[anchor..]);

    if out.len() >= dst.len() { return None; } // incompressible
    dst[..out.len()].copy_from_slice(&out);
    Some(out.len())
}

/// LZ4 block decompress. `dst` must be the original size. Returns true on success.
pub fn lz4_decompress(src: &[u8], dst: &mut [u8]) -> bool {
    let slen = src.len();
    let dlen = dst.len();
    let mut ip = 0usize; // src position
    let mut op = 0usize; // dst position

    macro_rules! read_byte { () => {{ if ip >= slen { return false; } let b = src[ip]; ip += 1; b }} }

    loop {
        let token = read_byte!();
        let mut lit_len = (token >> 4) as usize;
        if lit_len == 15 {
            loop { let b = read_byte!() as usize; lit_len += b; if b != 255 { break; } }
        }

        // Copy literals
        if ip + lit_len > slen || op + lit_len > dlen { return false; }
        dst[op..op + lit_len].copy_from_slice(&src[ip..ip + lit_len]);
        ip += lit_len; op += lit_len;

        // EOF: last sequence has no offset
        if ip >= slen { return op == dlen; }

        // Read 2-byte offset
        if ip + 2 > slen { return false; }
        let offset = src[ip] as usize | ((src[ip + 1] as usize) << 8);
        ip += 2;
        if offset == 0 || offset > op { return false; }

        // Match length
        let mut mat_len = (token & 0xF) as usize + LZ4_MIN_MATCH;
        if token & 0xF == 15 {
            loop { let b = read_byte!() as usize; mat_len += b; if b != 255 { break; } }
        }

        // Copy match (may overlap — use bytewise copy)
        if op + mat_len > dlen { return false; }
        let match_start = op - offset;
        for i in 0..mat_len {
            dst[op + i] = dst[match_start + i];
        }
        op += mat_len;
    }
}

// ── CRC32 (IEEE polynomial, for data integrity) ────────────────────────────

fn crc32_simple(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

// ── zram block device ─────────────────────────────────────────────────────
//
// A compressed in-memory block device. Exported as /dev/zram0.
// Each 4KB group of sectors is independently compressed via LZ4.
// The device stores a BTreeMap<u64, Vec<u8>> (block → compressed data).

pub struct ZramDevice {
    /// Maps block number (4KB aligned) to compressed page data.
    blocks: BTreeMap<u64, Vec<u8>>,
    pub total_sectors: u64,
    orig_bytes:   u64,
    comp_bytes:   u64,
}

impl ZramDevice {
    pub fn new(size_mb: u64) -> Self {
        ZramDevice {
            blocks:        BTreeMap::new(),
            total_sectors: size_mb * 1024 * 1024 / ZRAM_SECTOR_SIZE as u64,
            orig_bytes:    0,
            comp_bytes:    0,
        }
    }

    pub fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> bool {
        let n_sectors = buf.len() / ZRAM_SECTOR_SIZE;
        for s in 0..n_sectors as u64 {
            let sector = lba + s;
            let block  = sector / 8; // 8 sectors per 4KB block
            let s_off  = (sector % 8) as usize * ZRAM_SECTOR_SIZE;
            let b_off  = s as usize * ZRAM_SECTOR_SIZE;

            if let Some(compressed) = self.blocks.get(&block) {
                let mut page = [0u8; 4096];
                if !lz4_decompress(compressed, &mut page) { return false; }
                buf[b_off..b_off + ZRAM_SECTOR_SIZE].copy_from_slice(&page[s_off..s_off + ZRAM_SECTOR_SIZE]);
            } else {
                // Unwritten region: return zeros
                buf[b_off..b_off + ZRAM_SECTOR_SIZE].fill(0);
            }
        }
        true
    }

    pub fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> bool {
        let n_sectors = buf.len() / ZRAM_SECTOR_SIZE;
        let mut s = 0usize;
        while s < n_sectors {
            let sector = lba + s as u64;
            let block  = sector / 8;
            let s_off  = (sector % 8) as usize * ZRAM_SECTOR_SIZE;
            let mut page = [0u8; 4096];

            // Read-modify-write: decompress existing block if present
            if let Some(existing) = self.blocks.get(&block) {
                if !lz4_decompress(existing, &mut page) { return false; }
            }

            // How many sectors in this block can we fill in one pass?
            let first_s_in_block = (sector - block * 8) as usize;
            let can_fill = (8 - first_s_in_block).min(n_sectors - s);

            for i in 0..can_fill {
                let off = (first_s_in_block + i) * ZRAM_SECTOR_SIZE;
                let src_off = (s + i) * ZRAM_SECTOR_SIZE;
                page[off..off + ZRAM_SECTOR_SIZE].copy_from_slice(&buf[src_off..src_off + ZRAM_SECTOR_SIZE]);
            }

            // Compress and store
            let mut comp = alloc::vec![0u8; 4096 + 64];
            match lz4_compress(&page, &mut comp) {
                Some(comp_len) => {
                    let old_size = self.blocks.get(&block).map(|b| b.len()).unwrap_or(0);
                    comp.truncate(comp_len);
                    self.orig_bytes += 4096;
                    self.comp_bytes += comp_len as u64;
                    self.comp_bytes -= old_size as u64;
                    self.blocks.insert(block, comp);
                }
                None => {
                    // Store uncompressed (incompressible data)
                    self.blocks.insert(block, page.to_vec());
                    self.orig_bytes += 4096;
                    self.comp_bytes += 4096;
                }
            }

            s += can_fill;
        }
        true
    }

    pub fn compression_ratio(&self) -> f32 {
        if self.comp_bytes == 0 { return 1.0; }
        self.orig_bytes as f32 / self.comp_bytes as f32
    }

    pub fn used_blocks(&self) -> usize { self.blocks.len() }
}

// Global zram0 device
static ZRAM0: Mutex<Option<ZramDevice>> = Mutex::new(None);

pub fn zram_init(size_mb: u64) {
    *ZRAM0.lock() = Some(ZramDevice::new(size_mb));
    crate::klog!("zram: /dev/zram0 created ({} MB)", size_mb);
}

pub fn zram_read(lba: u64, buf: &mut [u8]) -> bool {
    ZRAM0.lock().as_mut().map(|z| z.read_sectors(lba, buf)).unwrap_or(false)
}

pub fn zram_write(lba: u64, buf: &[u8]) -> bool {
    ZRAM0.lock().as_mut().map(|z| z.write_sectors(lba, buf)).unwrap_or(false)
}

pub fn zram_sectors() -> u64 {
    ZRAM0.lock().as_ref().map(|z| z.total_sectors).unwrap_or(0)
}

pub fn zram_stats() -> Option<(u64, u64, f32)> {
    ZRAM0.lock().as_ref().map(|z| (z.orig_bytes, z.comp_bytes, z.compression_ratio()))
}

/// Default zram initialization: use 512 MB if system has > 2 GB RAM.
pub fn zram_init_default() {
    let total_mb = crate::memory::phys::total_frames() * 4096 / 1048576;
    if total_mb >= 2048 {
        zram_init(512);
    } else if total_mb >= 512 {
        zram_init(128);
    }
    // < 512 MB: skip zram to preserve precious physical RAM
}
