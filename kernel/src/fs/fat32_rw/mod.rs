/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::vfs::*;

const FAT_EOC:   u32 = 0x0FFF_FFF8;
const FAT_FREE:  u32 = 0x0000_0000;
const FAT_BAD:   u32 = 0x0FFF_FFF7;
const ATTR_DIR:  u8  = 0x10;
const ATTR_ARCH: u8  = 0x20;
const ATTR_LFN:  u8  = 0x0F;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Bpb {
    jmp:                [u8; 3],
    oem:                [u8; 8],
    bytes_per_sector:   u16,
    sectors_per_cluster: u8,
    reserved_sectors:   u16,
    num_fats:           u8,
    root_entry_count:   u16,
    total_sectors16:    u16,
    media:              u8,
    fat_size16:         u16,
    sectors_per_track:  u16,
    num_heads:          u16,
    hidden_sectors:     u32,
    total_sectors32:    u32,
    fat_size32:         u32,
    ext_flags:          u16,
    fs_version:         u16,
    root_cluster:       u32,
    fs_info:            u16,
    backup_boot:        u16,
    _reserved:          [u8; 12],
    drive_num:          u8,
    _reserved2:         u8,
    boot_sig:           u8,
    vol_id:             u32,
    vol_label:          [u8; 11],
    fs_type:            [u8; 8],
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct RawDirent {
    name:       [u8; 11],
    attr:       u8,
    _nt_res:    u8,
    crt_tenth:  u8,
    crt_time:   u16,
    crt_date:   u16,
    acc_date:   u16,
    clus_hi:    u16,
    wrt_time:   u16,
    wrt_date:   u16,
    clus_lo:    u16,
    file_size:  u32,
}

struct Fat32Inner {
    disk:        Vec<u8>,
    bps:         u32,
    spc:         u32,
    reserved:    u32,
    num_fats:    u32,
    fat_size:    u32,
    root_clus:   u32,
    data_start:  u32,
    dirty:       bool,
}

impl Fat32Inner {
    fn cluster_to_offset(&self, cluster: u32) -> usize {
        ((self.data_start + (cluster - 2) * self.spc) * self.bps) as usize
    }

    fn cluster_size(&self) -> usize {
        (self.spc * self.bps) as usize
    }

    fn fat_offset(&self, cluster: u32) -> usize {
        (self.reserved * self.bps) as usize + (cluster as usize) * 4
    }

    fn fat_get(&self, cluster: u32) -> u32 {
        let off = self.fat_offset(cluster);
        if off + 4 > self.disk.len() { return FAT_EOC; }
        u32::from_le_bytes(self.disk[off..off + 4].try_into().unwrap()) & 0x0FFF_FFFF
    }

    fn fat_set(&mut self, cluster: u32, val: u32) {
        for fat in 0..self.num_fats {
            let fat_start = (self.reserved + fat * self.fat_size) * self.bps;
            let off = fat_start as usize + cluster as usize * 4;
            if off + 4 <= self.disk.len() {
                self.disk[off..off + 4].copy_from_slice(&val.to_le_bytes());
            }
        }
        self.dirty = true;
    }

    fn alloc_cluster(&mut self) -> Option<u32> {
        let total = (self.fat_size * self.bps / 4) as u32;
        for c in 2..total {
            if self.fat_get(c) == FAT_FREE {
                self.fat_set(c, FAT_EOC);
                let off = self.cluster_to_offset(c);
                if off + self.cluster_size() <= self.disk.len() {
                    { let csz = self.cluster_size(); self.disk[off..off + csz].fill(0); }
                }
                return Some(c);
            }
        }
        None
    }

    fn free_chain(&mut self, start: u32) {
        let mut c = start;
        while c < FAT_EOC && c >= 2 {
            let next = self.fat_get(c);
            self.fat_set(c, FAT_FREE);
            c = next;
        }
    }

    fn chain_len(&self, start: u32) -> usize {
        let mut c = start;
        let mut n = 0usize;
        while c < FAT_EOC && c >= 2 {
            n += 1;
            c = self.fat_get(c);
        }
        n
    }

    fn read_chain(&self, start: u32) -> Vec<u8> {
        let csz = self.cluster_size();
        let mut out = Vec::new();
        let mut c = start;
        while c < FAT_EOC && c >= 2 {
            let off = self.cluster_to_offset(c);
            if off + csz <= self.disk.len() {
                out.extend_from_slice(&self.disk[off..off + csz]);
            }
            c = self.fat_get(c);
        }
        out
    }

    fn write_chain(&mut self, start: u32, data: &[u8]) -> Option<u32> {
        let csz = self.cluster_size();
        let needed = (data.len() + csz - 1) / csz;

        // Build existing chain
        let mut chain: Vec<u32> = Vec::new();
        let mut c = start;
        while c < FAT_EOC && c >= 2 {
            chain.push(c);
            c = self.fat_get(c);
        }

        // Extend chain if needed
        while chain.len() < needed {
            let new_c = self.alloc_cluster()?;
            if let Some(&last) = chain.last() {
                self.fat_set(last, new_c);
            }
            chain.push(new_c);
        }

        // Truncate excess clusters
        if chain.len() > needed && needed > 0 {
            self.fat_set(chain[needed - 1], FAT_EOC);
            for &excess in &chain[needed..] {
                self.fat_set(excess, FAT_FREE);
            }
        }

        // Write data
        for (i, &clus) in chain[..needed].iter().enumerate() {
            let src_off = i * csz;
            let src_end = (src_off + csz).min(data.len());
            let off = self.cluster_to_offset(clus);
            if off + csz <= self.disk.len() {
                let chunk_len = src_end - src_off;
                self.disk[off..off + chunk_len].copy_from_slice(&data[src_off..src_end]);
                if chunk_len < csz {
                    self.disk[off + chunk_len..off + csz].fill(0);
                }
            }
        }

        self.dirty = true;
        chain.first().copied().or(Some(0))
    }

    fn alloc_chain(&mut self, data: &[u8]) -> Option<u32> {
        let first = self.alloc_cluster()?;
        let csz = self.cluster_size();
        let needed = (data.len() + csz - 1).max(1) / csz;
        let mut chain = alloc::vec![first];

        for _ in 1..needed {
            let next = self.alloc_cluster()?;
            self.fat_set(*chain.last().unwrap(), next);
            chain.push(next);
        }

        for (i, &clus) in chain.iter().enumerate() {
            let off = self.cluster_to_offset(clus);
            let src_off = i * csz;
            let src_end = (src_off + csz).min(data.len());
            if off + csz <= self.disk.len() {
                if src_off < data.len() {
                    let n = src_end - src_off;
                    self.disk[off..off + n].copy_from_slice(&data[src_off..src_end]);
                }
            }
        }
        self.dirty = true;
        Some(first)
    }

    /// Parse directory entries returning 8.3 decoded names (no LFN).
    /// Used internally to get the 8.3 name for a given entry index.
    fn parse_entries(&self, dir_data: &[u8]) -> Vec<(String, u32, u32, bool)> {
        let mut out = Vec::new();
        let mut i = 0;
        while i + 32 <= dir_data.len() {
            let de = unsafe { &*(dir_data.as_ptr().add(i) as *const RawDirent) };
            if de.name[0] == 0x00 { break; }
            if de.name[0] == 0xE5 || de.attr == ATTR_LFN { i += 32; continue; }
            let name = decode_83(&de.name);
            let clus = ((u16::from_le(de.clus_hi) as u32) << 16) | u16::from_le(de.clus_lo) as u32;
            let size = u32::from_le(de.file_size);
            let is_dir = de.attr & ATTR_DIR != 0;
            out.push((name, clus, size, is_dir));
            i += 32;
        }
        out
    }

    /// Parse directory entries with Long File Name (LFN) support.
    /// LFN entries precede their 8.3 short-name entry and store Unicode
    /// name fragments in UCS-2LE across up to 20 LFN slots (255 chars max).
    /// Returns (long_name_or_83, cluster, size, is_dir).
    fn parse_entries_with_lfn(&self, dir_data: &[u8]) -> Vec<(String, u32, u32, bool)> {
        let mut out = Vec::new();
        let mut i = 0;
        // Accumulate LFN fragments; each LFN entry holds 13 UCS-2 chars.
        let mut lfn_parts: Vec<(u8, [u16; 13])> = Vec::new();

        while i + 32 <= dir_data.len() {
            let raw = &dir_data[i..i + 32];
            if raw[0] == 0x00 { break; }

            if raw[0] == 0xE5 {
                lfn_parts.clear();
                i += 32;
                continue;
            }

            let attr = raw[11];
            if attr == ATTR_LFN {
                // LFN slot: extract 13 UCS-2LE characters from offsets 1, 14, 28
                let order = raw[0] & 0x1F;
                let mut chars = [0u16; 13];
                let field_offsets = [(1usize, 5usize), (14, 6), (28, 2)];
                let mut ci = 0usize;
                for (off, cnt) in field_offsets.iter() {
                    for k in 0..*cnt {
                        let lo = raw[off + k * 2] as u16;
                        let hi = raw[off + k * 2 + 1] as u16;
                        chars[ci] = lo | (hi << 8);
                        ci += 1;
                    }
                }
                lfn_parts.push((order, chars));
                i += 32;
                continue;
            }

            // Regular 8.3 entry
            let de = unsafe { &*(raw.as_ptr() as *const RawDirent) };
            let clus = ((u16::from_le(de.clus_hi) as u32) << 16) | u16::from_le(de.clus_lo) as u32;
            let size = u32::from_le(de.file_size);
            let is_dir = attr & ATTR_DIR != 0;

            let name = if !lfn_parts.is_empty() {
                // Sort by order byte (1 = first segment)
                lfn_parts.sort_by_key(|(ord, _)| *ord);
                let mut ucs2: Vec<u16> = Vec::new();
                for (_, chars) in &lfn_parts {
                    for &c in chars.iter() {
                        if c == 0x0000 || c == 0xFFFF { break; }
                        ucs2.push(c);
                    }
                }
                let s: String = ucs2.iter()
                    .filter_map(|&c| char::from_u32(c as u32))
                    .collect();
                if s.is_empty() { decode_83(&de.name) } else { s }
            } else {
                decode_83(&de.name)
            };

            lfn_parts.clear();
            out.push((name, clus, size, is_dir));
            i += 32;
        }
        out
    }

    fn write_dirent_to_dir(&mut self, dir_cluster: u32, name_83: [u8; 11],
                           clus: u32, size: u32, is_dir: bool) -> bool {
        let csz = self.cluster_size();
        let mut c = dir_cluster;
        loop {
            let off = self.cluster_to_offset(c);
            let mut i = off;
            while i + 32 <= off + csz && i + 32 <= self.disk.len() {
                if self.disk[i] == 0xE5 || self.disk[i] == 0x00 {
                    let de = unsafe { &mut *(self.disk.as_mut_ptr().add(i) as *mut RawDirent) };
                    de.name      = name_83;
                    de.attr      = if is_dir { ATTR_DIR } else { ATTR_ARCH };
                    de.clus_hi   = ((clus >> 16) as u16).to_le();
                    de.clus_lo   = (clus as u16).to_le();
                    de.file_size = size.to_le();
                    de._nt_res   = 0;
                    de.crt_tenth = 0;
                    de.crt_time  = 0;
                    de.crt_date  = 0x4A21u16.to_le(); // 2016-01-01
                    de.acc_date  = de.crt_date;
                    de.wrt_time  = de.crt_time;
                    de.wrt_date  = de.crt_date;
                    self.dirty   = true;
                    return true;
                }
                i += 32;
            }
            let next = self.fat_get(c);
            if next >= FAT_EOC {
                // extend dir
                if let Some(new_c) = self.alloc_cluster() {
                    self.fat_set(c, new_c);
                    c = new_c;
                } else {
                    return false;
                }
            } else {
                c = next;
            }
        }
    }

    fn remove_dirent_from_dir(&mut self, dir_cluster: u32, name_83: [u8; 11]) -> bool {
        let csz = self.cluster_size();
        let mut c = dir_cluster;
        while c < FAT_EOC && c >= 2 {
            let off = self.cluster_to_offset(c);
            let mut i = off;
            while i + 32 <= off + csz && i + 32 <= self.disk.len() {
                let de = unsafe { &*(self.disk.as_ptr().add(i) as *const RawDirent) };
                if de.name == name_83 {
                    self.disk[i] = 0xE5;
                    self.dirty = true;
                    return true;
                }
                i += 32;
            }
            c = self.fat_get(c);
        }
        false
    }

    fn update_dirent_size(&mut self, dir_cluster: u32, name_83: [u8; 11],
                          new_clus: u32, new_size: u32) {
        let csz = self.cluster_size();
        let mut c = dir_cluster;
        while c < FAT_EOC && c >= 2 {
            let off = self.cluster_to_offset(c);
            let mut i = off;
            while i + 32 <= off + csz && i + 32 <= self.disk.len() {
                let de = unsafe { &mut *(self.disk.as_mut_ptr().add(i) as *mut RawDirent) };
                if de.name == name_83 {
                    de.clus_hi   = ((new_clus >> 16) as u16).to_le();
                    de.clus_lo   = (new_clus as u16).to_le();
                    de.file_size = new_size.to_le();
                    self.dirty   = true;
                    return;
                }
                i += 32;
            }
            c = self.fat_get(c);
        }
    }
}

fn decode_83(raw: &[u8; 11]) -> String {
    let name_end = raw[..8].iter().rposition(|&b| b != b' ').map(|i| i + 1).unwrap_or(0);
    let ext_end  = raw[8..].iter().rposition(|&b| b != b' ').map(|i| i + 1).unwrap_or(0);
    let mut s = String::from_utf8_lossy(&raw[..name_end]).to_ascii_lowercase();
    if ext_end > 0 {
        s.push('.');
        s.push_str(&String::from_utf8_lossy(&raw[8..8 + ext_end]).to_ascii_lowercase());
    }
    s
}

fn encode_83(name: &str) -> [u8; 11] {
    let mut out = [b' '; 11];
    let (base, ext) = name.rsplit_once('.').unwrap_or((name, ""));
    let base = base.to_ascii_uppercase();
    let ext  = ext.to_ascii_uppercase();
    for (i, b) in base.bytes().take(8).enumerate() { out[i] = b; }
    for (i, b) in ext.bytes().take(3).enumerate() { out[8 + i] = b; }
    out
}

pub struct Fat32RwFs {
    inner: Arc<Mutex<Fat32Inner>>,
    dev:   u64,
}

impl Fat32RwFs {
    pub fn new(disk: Vec<u8>) -> Option<Superblock> {
        if disk.len() < 512 { return None; }
        let bpb = unsafe { &*(disk.as_ptr() as *const Bpb) };
        let bps      = u16::from_le(bpb.bytes_per_sector) as u32;
        let spc      = bpb.sectors_per_cluster as u32;
        let reserved = u16::from_le(bpb.reserved_sectors) as u32;
        let num_fats = bpb.num_fats as u32;
        let fat_size = u32::from_le(bpb.fat_size32);
        let root_clus = u32::from_le(bpb.root_cluster);
        let data_start = reserved + num_fats * fat_size;

        let inner = Arc::new(Mutex::new(Fat32Inner {
            disk, bps, spc, reserved, num_fats,
            fat_size, root_clus, data_start, dirty: false,
        }));

        let ops: Arc<dyn SuperblockOps> = Arc::new(Fat32RwOps {
            inner: inner.clone(),
        });

        Some(Superblock { dev: 2, fs_type: String::from("fat32"), ops })
    }
}

struct Fat32RwOps {
    inner: Arc<Mutex<Fat32Inner>>,
}

impl SuperblockOps for Fat32RwOps {
    fn get_root(&self) -> Result<Inode, VfsError> {
        let root_clus = self.inner.lock().root_clus;
        self.make_inode(root_clus, 0, true, 1, root_clus, [b' '; 11])
    }

    fn sync(&self) {
        // In-memory FAT32 — dirty flag set, would flush to block device here
    }
}

impl Fat32RwOps {
    fn make_inode(&self, cluster: u32, size: u32, is_dir: bool, ino: u64, parent_clus: u32, name83: [u8; 11]) -> Result<Inode, VfsError> {
        let mode = if is_dir { S_IFDIR | 0o755 } else { S_IFREG | 0o644 };
        let ops  = Arc::new(Fat32RwInoOps {
            inner:       self.inner.clone(),
            cluster,
            is_dir,
            parent_clus,
            name83,
        });
        let sb = Arc::new(Superblock {
            dev: 2,
            fs_type: String::from("fat32"),
            ops: Arc::new(Fat32RwOps { inner: self.inner.clone() }),
        });
        Ok(Inode { ino, mode, uid: 0, gid: 0, size: size as u64,
                   atime: 0, mtime: 0, ctime: 0, ops, sb })
    }
}

struct Fat32RwInoOps {
    inner:       Arc<Mutex<Fat32Inner>>,
    cluster:     u32,
    is_dir:      bool,
    parent_clus: u32,
    /// 8.3 encoded name used to find this file's dirent in parent_clus.
    /// All zeros for the root directory.
    name83:      [u8; 11],
}

impl InodeOps for Fat32RwInoOps {
    fn read(&self, inode: &Inode, buf: &mut [u8], offset: u64) -> Result<usize, VfsError> {
        if self.is_dir { return Err(EISDIR); }
        let fs = self.inner.lock();
        let data = fs.read_chain(self.cluster);
        let start = offset as usize;
        if start >= data.len() { return Ok(0); }
        // Clamp to actual file size stored in inode (not raw cluster chain)
        let file_sz = inode.size as usize;
        let end = (start + buf.len()).min(file_sz).min(data.len());
        if end <= start { return Ok(0); }
        let n = end - start;
        buf[..n].copy_from_slice(&data[start..end]);
        Ok(n)
    }

    fn write(&self, _inode: &Inode, buf: &[u8], offset: u64) -> Result<usize, VfsError> {
        if self.is_dir { return Err(EISDIR); }
        let mut fs = self.inner.lock();
        let mut data = if self.cluster >= 2 { fs.read_chain(self.cluster) } else { alloc::vec![] };
        let new_end = offset as usize + buf.len();
        if new_end > data.len() { data.resize(new_end, 0); }
        data[offset as usize..offset as usize + buf.len()].copy_from_slice(buf);
        let new_clus = if self.cluster >= 2 {
            fs.write_chain(self.cluster, &data).unwrap_or(self.cluster)
        } else {
            fs.alloc_chain(&data).unwrap_or(0)
        };
        // Update directory entry with real cluster and size
        fs.update_dirent_size(self.parent_clus, self.name83, new_clus, data.len() as u32);
        Ok(buf.len())
    }

    fn readdir(&self, _inode: &Inode, _offset: u64) -> Result<Vec<DirEntry>, VfsError> {
        if !self.is_dir { return Err(ENOTDIR); }
        let fs = self.inner.lock();
        let data = fs.read_chain(self.cluster);
        let raw  = fs.parse_entries_with_lfn(&data);
        let mut entries = alloc::vec![
            DirEntry { name: String::from("."),  ino: self.cluster as u64, file_type: 4 },
            DirEntry { name: String::from(".."), ino: self.parent_clus as u64, file_type: 4 },
        ];
        for (name, clus, _, is_dir) in raw.iter() {
            entries.push(DirEntry {
                name: name.clone(),
                ino: *clus as u64,
                file_type: if *is_dir { 4 } else { 8 },
            });
        }
        Ok(entries)
    }

    fn lookup(&self, _inode: &Inode, name: &str) -> Result<Inode, VfsError> {
        if !self.is_dir { return Err(ENOTDIR); }
        let fs = self.inner.lock();
        let data = fs.read_chain(self.cluster);
        // Parse with LFN so long filenames resolve correctly
        let entries = fs.parse_entries_with_lfn(&data);
        // Also get the raw 8.3 entries for storing name83 per entry
        let raw83 = fs.parse_entries(&data);
        drop(fs);

        for (i, (ename, clus, size, is_dir)) in entries.iter().enumerate() {
            if ename.eq_ignore_ascii_case(name) {
                // Get the 8.3 name for this entry (same index in raw list)
                let name83 = raw83.get(i).map(|(n, _, _, _)| encode_83(n)).unwrap_or(encode_83(ename));
                let inode_ops = Arc::new(Fat32RwInoOps {
                    inner:       self.inner.clone(),
                    cluster:     *clus,
                    is_dir:      *is_dir,
                    parent_clus: self.cluster,
                    name83,
                });
                let sb = Arc::new(Superblock {
                    dev: 2, fs_type: String::from("fat32"),
                    ops: Arc::new(Fat32RwOps { inner: self.inner.clone() }),
                });
                let mode = if *is_dir { S_IFDIR | 0o755 } else { S_IFREG | 0o644 };
                return Ok(Inode {
                    ino: *clus as u64, mode, uid: 0, gid: 0,
                    size: *size as u64, atime: 0, mtime: 0, ctime: 0,
                    ops: inode_ops, sb,
                });
            }
        }
        Err(ENOENT)
    }

    fn create(&self, _inode: &Inode, name: &str, mode: u32) -> Result<Inode, VfsError> {
        let name83 = encode_83(name);
        let mut fs  = self.inner.lock();
        let new_clus = fs.alloc_cluster().ok_or(ENOSPC)?;
        fs.write_dirent_to_dir(self.cluster, name83, new_clus, 0, false);
        drop(fs);
        let ino_ops = Arc::new(Fat32RwInoOps {
            inner: self.inner.clone(), cluster: new_clus,
            is_dir: false, parent_clus: self.cluster,
            name83,
        });
        let sb = Arc::new(Superblock { dev: 2, fs_type: String::from("fat32"),
            ops: Arc::new(Fat32RwOps { inner: self.inner.clone() }) });
        Ok(Inode { ino: new_clus as u64, mode: S_IFREG | (mode & 0o777),
                   uid: 0, gid: 0, size: 0, atime: 0, mtime: 0, ctime: 0, ops: ino_ops, sb })
    }

    fn mkdir(&self, _inode: &Inode, name: &str, mode: u32) -> Result<Inode, VfsError> {
        let name83 = encode_83(name);
        let mut fs  = self.inner.lock();
        let new_clus = fs.alloc_cluster().ok_or(ENOSPC)?;
        // Write . and .. entries into the new cluster
        let csz = fs.cluster_size();
        let off = fs.cluster_to_offset(new_clus);
        if off + 64 <= fs.disk.len() {
            let de_dot = unsafe { &mut *(fs.disk.as_mut_ptr().add(off) as *mut RawDirent) };
            de_dot.name = *b".          ";
            de_dot.attr = ATTR_DIR;
            de_dot.clus_lo = (new_clus as u16).to_le();
            de_dot.clus_hi = ((new_clus >> 16) as u16).to_le();
            de_dot.file_size = 0;
            let de_dotdot = unsafe { &mut *(fs.disk.as_mut_ptr().add(off + 32) as *mut RawDirent) };
            de_dotdot.name = *b"..         ";
            de_dotdot.attr = ATTR_DIR;
            de_dotdot.clus_lo = (self.cluster as u16).to_le();
            de_dotdot.clus_hi = ((self.cluster >> 16) as u16).to_le();
            de_dotdot.file_size = 0;
            fs.dirty = true;
        }
        fs.write_dirent_to_dir(self.cluster, name83, new_clus, 0, true);
        drop(fs);
        let ino_ops = Arc::new(Fat32RwInoOps {
            inner: self.inner.clone(), cluster: new_clus,
            is_dir: true, parent_clus: self.cluster,
            name83,
        });
        let sb = Arc::new(Superblock { dev: 2, fs_type: String::from("fat32"),
            ops: Arc::new(Fat32RwOps { inner: self.inner.clone() }) });
        Ok(Inode { ino: new_clus as u64, mode: S_IFDIR | (mode & 0o777),
                   uid: 0, gid: 0, size: 0, atime: 0, mtime: 0, ctime: 0, ops: ino_ops, sb })
    }

    fn unlink(&self, _inode: &Inode, name: &str) -> Result<(), VfsError> {
        let name83 = encode_83(name);
        let mut fs  = self.inner.lock();
        let data = fs.read_chain(self.cluster);
        let entries = fs.parse_entries_with_lfn(&data);
        for (ename, clus, _, is_dir) in &entries {
            if ename.eq_ignore_ascii_case(name) {
                if *is_dir { return Err(EISDIR); }
                if *clus >= 2 { fs.free_chain(*clus); }
                fs.remove_dirent_from_dir(self.cluster, name83);
                return Ok(());
            }
        }
        Err(ENOENT)
    }

    fn rmdir(&self, _inode: &Inode, name: &str) -> Result<(), VfsError> {
        let name83 = encode_83(name);
        let mut fs  = self.inner.lock();
        let data = fs.read_chain(self.cluster);
        let entries = fs.parse_entries_with_lfn(&data);
        for (ename, clus, _, is_dir) in &entries {
            if ename.eq_ignore_ascii_case(name) {
                if !is_dir { return Err(ENOTDIR); }
                // Check directory is empty (only . and .. allowed)
                if *clus >= 2 {
                    let dir_data = fs.read_chain(*clus);
                    let children = fs.parse_entries(&dir_data);
                    // parse_entries skips . and .. (they are skipped by name[0] == '.')
                    // Actually decode_83 for "." returns "." which we need to handle:
                    let non_dot = children.iter().filter(|(n, _, _, _)| n != "." && n != "..").count();
                    if non_dot > 0 { return Err(ENOTEMPTY); }
                    fs.free_chain(*clus);
                }
                fs.remove_dirent_from_dir(self.cluster, name83);
                return Ok(());
            }
        }
        Err(ENOENT)
    }

    fn truncate(&self, _inode: &Inode, size: u64) -> Result<(), VfsError> {
        if self.is_dir { return Err(EISDIR); }
        let mut fs = self.inner.lock();
        if size == 0 {
            if self.cluster >= 2 { fs.free_chain(self.cluster); }
            fs.update_dirent_size(self.parent_clus, self.name83, 0, 0);
            return Ok(());
        }
        let mut data = fs.read_chain(self.cluster);
        data.resize(size as usize, 0);
        if self.cluster >= 2 {
            let new_clus = fs.write_chain(self.cluster, &data).unwrap_or(self.cluster);
            fs.update_dirent_size(self.parent_clus, self.name83, new_clus, size as u32);
        } else if !data.is_empty() {
            let new_clus = fs.alloc_chain(&data).unwrap_or(0);
            fs.update_dirent_size(self.parent_clus, self.name83, new_clus, size as u32);
        }
        Ok(())
    }
}
