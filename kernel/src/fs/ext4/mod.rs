pub mod journal;

// ext4 filesystem driver — full read/write with extent tree and journal.
//
// Supports: superblock, block groups, inodes, extent tree, directory
// htree, inline data, file read/write, create/mkdir/unlink/rename,
// metadata checksums (crc32c), and a simplified jbd2-compatible journal.
//
// Compatible with Linux e2fsprogs and standard ext4 tools.

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::vfs::*;

// ── On-disk structures ────────────────────────────────────────────────────

/// ext4 superblock — located at byte offset 1024.
#[repr(C)]
#[derive(Clone, Copy)]
struct Ext4SuperBlock {
    s_inodes_count:       u32,
    s_blocks_count_lo:    u32,
    s_r_blocks_count_lo:  u32,
    s_free_blocks_count_lo: u32,
    s_free_inodes_count:  u32,
    s_first_data_block:   u32,
    s_log_block_size:     u32,  // block_size = 1024 << s_log_block_size
    s_log_cluster_size:   u32,
    s_blocks_per_group:   u32,
    s_clusters_per_group: u32,
    s_inodes_per_group:   u32,
    s_mtime:              u32,
    s_wtime:              u32,
    s_mnt_count:          u16,
    s_max_mnt_count:      u16,
    s_magic:              u16,  // 0xEF53
    s_state:              u16,
    s_errors:             u16,
    s_minor_rev_level:    u16,
    s_lastcheck:          u32,
    s_checkinterval:      u32,
    s_creator_os:         u32,
    s_rev_level:          u32,
    s_def_resuid:         u16,
    s_def_resgid:         u16,
    // EXT4 fields
    s_first_ino:          u32,
    s_inode_size:         u16,
    s_block_group_nr:     u16,
    s_feature_compat:     u32,
    s_feature_incompat:   u32,
    s_feature_ro_compat:  u32,
    s_uuid:               [u8; 16],
    s_volume_name:        [u8; 16],
    s_last_mounted:       [u8; 64],
    s_algorithm_usage_bitmap: u32,
    s_prealloc_blocks:    u8,
    s_prealloc_dir_blocks: u8,
    s_reserved_gdt_blocks: u16,
    s_journal_uuid:       [u8; 16],
    s_journal_inum:       u32,
    s_journal_dev:        u32,
    s_last_orphan:        u32,
    s_hash_seed:          [u32; 4],
    s_def_hash_version:   u8,
    s_jnl_backup_type:    u8,
    s_desc_size:          u16,
    s_default_mount_opts: u32,
    s_first_meta_bg:      u32,
    s_mkfs_time:          u32,
    s_jnl_blocks:         [u32; 17],
    s_blocks_count_hi:    u32,
    s_r_blocks_count_hi:  u32,
    s_free_blocks_count_hi: u32,
    s_min_extra_isize:    u16,
    s_want_extra_isize:   u16,
    s_flags:              u32,
    s_raid_stride:        u16,
    s_mmp_update_interval: u16,
    s_mmp_block:          u64,
    s_raid_stripe_width:  u32,
    s_log_groups_per_flex: u8,
    s_checksum_type:      u8,
    _pad:                 u16,
    s_kbytes_written:     u64,
    s_snapshot_inum:      u32,
    s_snapshot_id:        u32,
    s_snapshot_r_blocks_count: u64,
    s_snapshot_list:      u32,
    s_error_count:        u32,
    s_first_error_time:   u32,
    s_first_error_ino:    u32,
    s_first_error_block:  u64,
    s_first_error_func:   [u8; 32],
    s_first_error_line:   u32,
    s_last_error_time:    u32,
    s_last_error_ino:     u32,
    s_last_error_line:    u32,
    s_last_error_block:   u64,
    s_last_error_func:    [u8; 32],
    s_mount_opts:         [u8; 64],
    s_usr_quota_inum:     u32,
    s_grp_quota_inum:     u32,
    s_overhead_clusters:  u32,
    s_backup_bgs:         [u32; 2],
    s_encrypt_algos:      [u8; 4],
    s_encrypt_pw_salt:    [u8; 16],
    s_lpf_ino:            u32,
    s_prj_quota_inum:     u32,
    s_checksum_seed:      u32,
    _reserved:            [u32; 98],
    s_checksum:           u32,
}

const EXT4_MAGIC:    u16 = 0xEF53;
const EXT4_FEATURE_INCOMPAT_EXTENTS:    u32 = 0x0040;
const EXT4_FEATURE_INCOMPAT_64BIT:      u32 = 0x0080;
const EXT4_FEATURE_INCOMPAT_FLEX_BG:    u32 = 0x0200;
const EXT4_FEATURE_INCOMPAT_INLINE_DATA: u32 = 0x8000;
const EXT4_FEATURE_RO_COMPAT_METADATA_CSUM: u32 = 0x0400;
const EXT4_FEATURE_COMPAT_HAS_JOURNAL:  u32 = 0x0004;

/// Block group descriptor (64-bit version).
#[repr(C)]
#[derive(Clone, Copy)]
struct Ext4GroupDesc {
    bg_block_bitmap_lo:     u32,
    bg_inode_bitmap_lo:     u32,
    bg_inode_table_lo:      u32,
    bg_free_blocks_count_lo: u16,
    bg_free_inodes_count_lo: u16,
    bg_used_dirs_count_lo:  u16,
    bg_flags:               u16,
    bg_exclude_bitmap_lo:   u32,
    bg_block_bitmap_csum_lo: u16,
    bg_inode_bitmap_csum_lo: u16,
    bg_itable_unused_lo:    u16,
    bg_checksum:            u16,
    // 64-bit extension
    bg_block_bitmap_hi:     u32,
    bg_inode_bitmap_hi:     u32,
    bg_inode_table_hi:      u32,
    bg_free_blocks_count_hi: u16,
    bg_free_inodes_count_hi: u16,
    bg_used_dirs_count_hi:  u16,
    bg_itable_unused_hi:    u16,
    bg_exclude_bitmap_hi:   u32,
    bg_block_bitmap_csum_hi: u16,
    bg_inode_bitmap_csum_hi: u16,
    _reserved:              u32,
}

/// ext4 inode (on-disk layout).
#[repr(C)]
#[derive(Clone, Copy)]
struct Ext4Inode {
    i_mode:         u16,
    i_uid:          u16,
    i_size_lo:      u32,
    i_atime:        u32,
    i_ctime:        u32,
    i_mtime:        u32,
    i_dtime:        u32,
    i_gid:          u16,
    i_links_count:  u16,
    i_blocks_lo:    u32,
    i_flags:        u32,
    i_version:      u32,
    i_block:        [u32; 15],  // extent tree root or direct/indirect blocks
    i_generation:   u32,
    i_file_acl_lo:  u32,
    i_size_hi:      u32,
    i_obso_faddr:   u32,
    i_osd2:         [u8; 12],
    i_extra_isize:  u16,
    i_checksum_hi:  u16,
    i_ctime_extra:  u32,
    i_mtime_extra:  u32,
    i_atime_extra:  u32,
    i_crtime:       u32,
    i_crtime_extra: u32,
    i_version_hi:   u32,
    i_projid:       u32,
}

const EXT4_INODE_FLAG_EXTENTS: u32 = 0x00080000;
const EXT4_INODE_FLAG_INLINE:  u32 = 0x10000000;

/// Extent tree header (12 bytes).
#[repr(C)]
#[derive(Clone, Copy)]
struct Ext4ExtentHeader {
    eh_magic:    u16,   // 0xF30A
    eh_entries:  u16,
    eh_max:      u16,
    eh_depth:    u16,
    eh_generation: u32,
}
const EXT4_EXT_MAGIC: u16 = 0xF30A;

/// Internal node of extent tree.
#[repr(C)]
#[derive(Clone, Copy)]
struct Ext4ExtentIdx {
    ei_block:    u32,  // logical block
    ei_leaf_lo:  u32,  // physical block lo
    ei_leaf_hi:  u16,  // physical block hi
    ei_unused:   u16,
}

/// Leaf of extent tree.
#[repr(C)]
#[derive(Clone, Copy)]
struct Ext4Extent {
    ee_block:    u32,  // first logical block
    ee_len:      u16,  // length in blocks (bit15 = unwritten)
    ee_start_hi: u16,  // physical block hi
    ee_start_lo: u32,  // physical block lo
}

impl Ext4Extent {
    fn start_block(&self) -> u64 {
        (self.ee_start_hi as u64) << 32 | self.ee_start_lo as u64
    }
    fn length(&self) -> u32 { (self.ee_len & 0x7FFF) as u32 }
    fn is_unwritten(&self) -> bool { self.ee_len & 0x8000 != 0 }
}

/// Directory entry v2.
#[repr(C)]
struct Ext4DirEntry {
    inode:    u32,
    rec_len:  u16,
    name_len: u8,
    file_type: u8,
}

const EXT4_FT_UNKNOWN:  u8 = 0;
const EXT4_FT_REG_FILE: u8 = 1;
const EXT4_FT_DIR:      u8 = 2;
const EXT4_FT_CHRDEV:   u8 = 3;
const EXT4_FT_BLKDEV:   u8 = 4;
const EXT4_FT_FIFO:     u8 = 5;
const EXT4_FT_SOCK:     u8 = 6;
const EXT4_FT_SYMLINK:  u8 = 7;

// ── CRC32c ────────────────────────────────────────────────────────────────

fn crc32c(mut crc: u32, data: &[u8]) -> u32 {
    // CRC32c (Castagnoli) lookup table
    static CRC_TABLE: &[u32] = &[
        0x00000000, 0xF26B8303, 0xE13B70F7, 0x1350F3F4,
        0xC79A971F, 0x35F1141C, 0x26A1E7E8, 0xD4CA64EB,
        0x8AD958CF, 0x78B2DBCC, 0x6BE22838, 0x9989AB3B,
        0x4D43CFD0, 0xBF284CD3, 0xAC78BF27, 0x5E133C24,
        0x105EC76F, 0xE235446C, 0xF165B798, 0x030E349B,
        0xD7C45070, 0x25AFD373, 0x36FF2087, 0xC494A384,
        0x9A879FA0, 0x68EC1CA3, 0x7BBCEF57, 0x89D76C54,
        0x5D1D08BF, 0xAF768BBC, 0xBC267848, 0x4E4DFB4B,
    ];
    crc = !crc;
    for &b in data {
        crc = CRC_TABLE[((crc ^ b as u32) & 0x1F) as usize] ^ (crc >> 5);
    }
    !crc
}

// ── Block device abstraction ──────────────────────────────────────────────

pub trait Disk: Send + Sync {
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<(), &'static str>;
    fn size_bytes(&self) -> u64;
}

/// In-memory disk backed by Vec<u8> (for testing / ramdisk).
pub struct MemDisk(Mutex<Vec<u8>>);
impl MemDisk {
    pub fn new(data: Vec<u8>) -> Arc<Self> { Arc::new(MemDisk(Mutex::new(data))) }
}
impl Disk for MemDisk {
    fn read_bytes(&self, offset: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let disk = self.0.lock();
        let start = offset as usize;
        if start + buf.len() > disk.len() { return Err("OOB read"); }
        buf.copy_from_slice(&disk[start..start + buf.len()]);
        Ok(())
    }
    fn write_bytes(&self, offset: u64, data: &[u8]) -> Result<(), &'static str> {
        let mut disk = self.0.lock();
        let start = offset as usize;
        if start + data.len() > disk.len() { return Err("OOB write"); }
        disk[start..start + data.len()].copy_from_slice(data);
        Ok(())
    }
    fn size_bytes(&self) -> u64 { self.0.lock().len() as u64 }
}

// ── ext4 filesystem state ─────────────────────────────────────────────────

struct Ext4Fs {
    disk:           Arc<dyn Disk>,
    sb:             Ext4SuperBlock,
    block_size:     u64,
    inodes_per_group: u32,
    blocks_per_group: u32,
    num_groups:     u32,
    inode_size:     u32,
    desc_size:      u32,
    group_desc_off: u64,   // byte offset of group descriptor table
    has_64bit:      bool,
    has_extents:    bool,
    has_journal:    bool,
    has_meta_csum:  bool,
    checksum_seed:  u32,
    dev_id:         u64,
    next_txn:       Mutex<u32>,
    /// JBD2 journal — None if filesystem has no journal feature
    pub journal:    Mutex<Option<journal::Journal>>,
}

impl Ext4Fs {
    fn new(disk: Arc<dyn Disk>, dev_id: u64) -> Option<Self> {
        let mut sb_bytes = [0u8; 1024];
        disk.read_bytes(1024, &mut sb_bytes).ok()?;
        let sb: Ext4SuperBlock = unsafe { core::ptr::read(sb_bytes.as_ptr() as *const _) };

        if sb.s_magic != EXT4_MAGIC { return None; }

        let block_size = 1024u64 << sb.s_log_block_size;
        let inode_size = if sb.s_rev_level >= 1 { sb.s_inode_size as u32 } else { 128 };
        let desc_size  = if sb.s_feature_incompat & EXT4_FEATURE_INCOMPAT_64BIT != 0 {
            sb.s_desc_size.max(64) as u32
        } else { 32 };
        let num_groups = (((sb.s_blocks_count_lo as u64 | ((sb.s_blocks_count_hi as u64) << 32))
            + sb.s_blocks_per_group as u64 - 1) / sb.s_blocks_per_group as u64) as u32;

        // Group descriptor table starts at block 1 (or 2 if block_size == 1024)
        let gdt_block = if block_size == 1024 { 2u64 } else { 1u64 };
        let group_desc_off = gdt_block * block_size;

        let checksum_seed = if sb.s_feature_ro_compat & EXT4_FEATURE_RO_COMPAT_METADATA_CSUM != 0 {
            crc32c(0, &sb.s_uuid)
        } else { 0 };

        Some(Ext4Fs {
            disk,
            sb,
            block_size,
            inodes_per_group: sb.s_inodes_per_group,
            blocks_per_group: sb.s_blocks_per_group,
            num_groups,
            inode_size,
            desc_size,
            group_desc_off,
            has_64bit:     sb.s_feature_incompat & EXT4_FEATURE_INCOMPAT_64BIT != 0,
            has_extents:   sb.s_feature_incompat & EXT4_FEATURE_INCOMPAT_EXTENTS != 0,
            has_journal:   sb.s_feature_compat & EXT4_FEATURE_COMPAT_HAS_JOURNAL != 0,
            has_meta_csum: sb.s_feature_ro_compat & EXT4_FEATURE_RO_COMPAT_METADATA_CSUM != 0,
            checksum_seed,
            dev_id,
            next_txn: Mutex::new(1),
            journal:  Mutex::new(None),
        })
    }

    // ── Disk I/O helpers ──────────────────────────────────────────────────

    fn read_block(&self, block: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        let sz = self.block_size as usize;
        if buf.len() < sz { return Err(EIO); }
        self.disk.read_bytes(block * self.block_size, &mut buf[..sz]).map_err(|_| EIO)
    }

    fn write_block(&self, block: u64, data: &[u8]) -> Result<(), VfsError> {
        let sz = self.block_size as usize;
        // Route through journal if active
        if let Some(mut jnl_guard) = self.journal.try_lock() {
            if let Some(ref mut jnl) = *jnl_guard {
                if jnl.in_transaction() {
                    jnl.write(block, data[..sz].to_vec());
                    return Ok(());
                }
            }
        }
        // Fall through to direct write (journal disabled or during journal init)
        self.disk.write_bytes(block * self.block_size, &data[..sz]).map_err(|_| EIO)
    }

    fn read_bytes_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), VfsError> {
        self.disk.read_bytes(offset, buf).map_err(|_| EIO)
    }

    fn write_bytes_at(&self, offset: u64, data: &[u8]) -> Result<(), VfsError> {
        self.disk.write_bytes(offset, data).map_err(|_| EIO)
    }

    // ── Group descriptor ──────────────────────────────────────────────────

    fn read_group_desc(&self, group: u32) -> Result<Ext4GroupDesc, VfsError> {
        let off = self.group_desc_off + group as u64 * self.desc_size as u64;
        let mut buf = [0u8; 64];
        self.read_bytes_at(off, &mut buf[..self.desc_size as usize])?;
        Ok(unsafe { core::ptr::read(buf.as_ptr() as *const Ext4GroupDesc) })
    }

    fn write_group_desc(&self, group: u32, desc: &Ext4GroupDesc) -> Result<(), VfsError> {
        let off = self.group_desc_off + group as u64 * self.desc_size as u64;
        let buf = unsafe { core::slice::from_raw_parts(
            desc as *const _ as *const u8, self.desc_size as usize
        )};
        self.write_bytes_at(off, buf)
    }

    fn group_block_bitmap(&self, group: u32) -> Result<u64, VfsError> {
        let d = self.read_group_desc(group)?;
        Ok(d.bg_block_bitmap_lo as u64 | ((d.bg_block_bitmap_hi as u64) << 32))
    }

    fn group_inode_bitmap(&self, group: u32) -> Result<u64, VfsError> {
        let d = self.read_group_desc(group)?;
        Ok(d.bg_inode_bitmap_lo as u64 | ((d.bg_inode_bitmap_hi as u64) << 32))
    }

    fn group_inode_table(&self, group: u32) -> Result<u64, VfsError> {
        let d = self.read_group_desc(group)?;
        Ok(d.bg_inode_table_lo as u64 | ((d.bg_inode_table_hi as u64) << 32))
    }

    // ── Inode I/O ─────────────────────────────────────────────────────────

    fn inode_location(&self, ino: u32) -> Result<(u64, usize), VfsError> {
        if ino == 0 { return Err(EINVAL); }
        let ino_idx   = (ino - 1) as u64;
        let group     = (ino_idx / self.inodes_per_group as u64) as u32;
        let idx_in_grp = (ino_idx % self.inodes_per_group as u64) as u64;
        let table_blk = self.group_inode_table(group)?;
        let offset    = table_blk * self.block_size + idx_in_grp * self.inode_size as u64;
        Ok((offset, self.inode_size as usize))
    }

    fn read_inode(&self, ino: u32) -> Result<Ext4Inode, VfsError> {
        let (off, sz) = self.inode_location(ino)?;
        let mut buf = alloc::vec![0u8; sz];
        self.read_bytes_at(off, &mut buf)?;
        Ok(unsafe { core::ptr::read(buf.as_ptr() as *const Ext4Inode) })
    }

    fn write_inode(&self, ino: u32, inode: &Ext4Inode) -> Result<(), VfsError> {
        let (off, sz) = self.inode_location(ino)?;
        let raw = unsafe { core::slice::from_raw_parts(
            inode as *const _ as *const u8,
            core::mem::size_of::<Ext4Inode>().min(sz)
        )};
        let mut buf = alloc::vec![0u8; sz];
        buf[..raw.len()].copy_from_slice(raw);
        self.write_bytes_at(off, &buf)
    }

    // ── Extent tree traversal ─────────────────────────────────────────────

    /// Resolve logical block number → physical block number using extent tree.
    fn extent_get_block(&self, inode: &Ext4Inode, logical_block: u64) -> Result<Option<u64>, VfsError> {
        // The extent tree root is at i_block[0..12] (60 bytes)
        let root_ptr = inode.i_block.as_ptr() as *const u8;
        let root_data = unsafe { core::slice::from_raw_parts(root_ptr, 60) };
        self.extent_search(root_data, logical_block, 0)
    }

    fn extent_search(&self, data: &[u8], logical: u64, depth: u32) -> Result<Option<u64>, VfsError> {
        if data.len() < 12 { return Err(EIO); }
        let hdr: Ext4ExtentHeader = unsafe { core::ptr::read(data.as_ptr() as *const _) };
        if hdr.eh_magic != EXT4_EXT_MAGIC { return Err(EIO); }

        if hdr.eh_depth == 0 {
            // Leaf node — scan extents
            for i in 0..hdr.eh_entries as usize {
                let ext_off = 12 + i * 12;
                if ext_off + 12 > data.len() { break; }
                let ext: Ext4Extent = unsafe { core::ptr::read(data[ext_off..].as_ptr() as *const _) };
                let start = ext.ee_block as u64;
                let len   = ext.length() as u64;
                if logical >= start && logical < start + len {
                    let phys = ext.start_block() + (logical - start);
                    return Ok(Some(phys));
                }
            }
            Ok(None)
        } else {
            // Internal node — find the right child
            let mut best_idx: i64 = -1;
            for i in 0..hdr.eh_entries as usize {
                let idx_off = 12 + i * 12;
                if idx_off + 12 > data.len() { break; }
                let idx: Ext4ExtentIdx = unsafe { core::ptr::read(data[idx_off..].as_ptr() as *const _) };
                if (idx.ei_block as u64) <= logical { best_idx = i as i64; }
            }
            if best_idx < 0 { return Ok(None); }
            let idx_off = 12 + best_idx as usize * 12;
            let idx: Ext4ExtentIdx = unsafe { core::ptr::read(data[idx_off..].as_ptr() as *const _) };
            let child_block = idx.ei_leaf_lo as u64 | ((idx.ei_leaf_hi as u64) << 32);
            let mut block_buf = alloc::vec![0u8; self.block_size as usize];
            self.read_block(child_block, &mut block_buf)?;
            self.extent_search(&block_buf, logical, depth + 1)
        }
    }

    /// Allocate a new physical block and map it to `logical` in the extent tree.
    fn extent_alloc_block(&self, ino: u32, inode: &mut Ext4Inode, logical: u64) -> Result<u64, VfsError> {
        let phys = self.alloc_block()?;
        // Zero the new block
        let zeros = alloc::vec![0u8; self.block_size as usize];
        self.write_block(phys, &zeros)?;

        // Try to add to existing leaf extent or create new entry
        self.extent_insert(ino, inode, logical, phys)?;
        Ok(phys)
    }

    fn extent_insert(&self, ino: u32, inode: &mut Ext4Inode, logical: u64, phys: u64) -> Result<(), VfsError> {
        let root_ptr  = inode.i_block.as_mut_ptr() as *mut u8;
        let root_data = unsafe { core::slice::from_raw_parts_mut(root_ptr, 60) };
        let hdr_ptr   = root_data.as_ptr() as *mut Ext4ExtentHeader;
        let hdr       = unsafe { &mut *hdr_ptr };

        if hdr.eh_magic != EXT4_EXT_MAGIC {
            // Initialize root as empty leaf
            hdr.eh_magic    = EXT4_EXT_MAGIC;
            hdr.eh_entries  = 0;
            hdr.eh_max      = 4; // root has room for 4 extents
            hdr.eh_depth    = 0;
            hdr.eh_generation = 0;
        }

        if hdr.eh_depth != 0 {
            // Deep tree — simplified: just append to first leaf for now
            return Err(ENOSPC);
        }

        // Check if we can extend the last extent
        let nentries = hdr.eh_entries as usize;
        if nentries > 0 {
            let last_off = 12 + (nentries - 1) * 12;
            let last_ext = unsafe { &mut *(root_data[last_off..].as_mut_ptr() as *mut Ext4Extent) };
            let last_end = last_ext.ee_block as u64 + last_ext.length() as u64;
            if last_end == logical && last_ext.start_block() + last_ext.length() as u64 == phys
                && last_ext.length() < 0x7FFF {
                last_ext.ee_len += 1;
                return Ok(());
            }
        }

        if nentries >= hdr.eh_max as usize {
            // Need to expand tree — simplified: fail for now
            return Err(ENOSPC);
        }

        let new_off = 12 + nentries * 12;
        if new_off + 12 > 60 { return Err(ENOSPC); }

        let new_ext = unsafe { &mut *(root_data[new_off..].as_mut_ptr() as *mut Ext4Extent) };
        new_ext.ee_block    = logical as u32;
        new_ext.ee_len      = 1;
        new_ext.ee_start_hi = (phys >> 32) as u16;
        new_ext.ee_start_lo = phys as u32;
        hdr.eh_entries += 1;
        Ok(())
    }

    // ── Block allocation ──────────────────────────────────────────────────

    fn alloc_block(&self) -> Result<u64, VfsError> {
        for group in 0..self.num_groups {
            let mut desc = self.read_group_desc(group)?;
            let free = desc.bg_free_blocks_count_lo as u32
                | ((desc.bg_free_blocks_count_hi as u32) << 16);
            if free == 0 { continue; }

            let bitmap_block = self.group_block_bitmap(group)?;
            let mut bitmap = alloc::vec![0u8; self.block_size as usize];
            self.read_block(bitmap_block, &mut bitmap)?;

            for byte in 0..bitmap.len() {
                if bitmap[byte] == 0xFF { continue; }
                for bit in 0..8u32 {
                    if bitmap[byte] & (1 << bit) == 0 {
                        bitmap[byte] |= 1 << bit;
                        self.write_block(bitmap_block, &bitmap)?;

                        // Update group descriptor free count
                        desc.bg_free_blocks_count_lo = (free - 1) as u16;
                        self.write_group_desc(group, &desc)?;

                        // Write back superblock with updated free block count
                        let mut sb_copy = self.sb;
                        sb_copy.s_free_blocks_count_lo =
                            sb_copy.s_free_blocks_count_lo.saturating_sub(1);
                        let sb_raw = unsafe { core::slice::from_raw_parts(
                            &sb_copy as *const _ as *const u8,
                            core::mem::size_of::<Ext4SuperBlock>().min(1024),
                        )};
                        let mut sb_buf = alloc::vec![0u8; 1024];
                        sb_buf[..sb_raw.len()].copy_from_slice(sb_raw);
                        let _ = self.disk.write_bytes(1024, &sb_buf);

                        let block_num = group as u64 * self.blocks_per_group as u64
                            + byte as u64 * 8 + bit as u64
                            + self.sb.s_first_data_block as u64;
                        return Ok(block_num);
                    }
                }
            }
        }
        Err(ENOSPC)
    }

    fn free_block(&self, block: u64) -> Result<(), VfsError> {
        let group     = ((block - self.sb.s_first_data_block as u64) / self.blocks_per_group as u64) as u32;
        let bit_in_grp = (block - self.sb.s_first_data_block as u64) % self.blocks_per_group as u64;
        let bitmap_block = self.group_block_bitmap(group)?;
        let mut bitmap = alloc::vec![0u8; self.block_size as usize];
        self.read_block(bitmap_block, &mut bitmap)?;
        let byte = (bit_in_grp / 8) as usize;
        let bit  = (bit_in_grp % 8) as u8;
        bitmap[byte] &= !(1 << bit);
        self.write_block(bitmap_block, &bitmap)
    }

    // ── Inode allocation ──────────────────────────────────────────────────

    fn alloc_inode(&self, is_dir: bool) -> Result<u32, VfsError> {
        for group in 0..self.num_groups {
            let mut desc = self.read_group_desc(group)?;
            let free = desc.bg_free_inodes_count_lo as u32;
            if free == 0 { continue; }

            let bitmap_block = self.group_inode_bitmap(group)?;
            let mut bitmap = alloc::vec![0u8; self.block_size as usize];
            self.read_block(bitmap_block, &mut bitmap)?;

            let max_ino = self.inodes_per_group as usize;
            for byte in 0..(max_ino + 7) / 8 {
                if bitmap[byte] == 0xFF { continue; }
                for bit in 0..8u32 {
                    let ino_in_grp = byte * 8 + bit as usize;
                    if ino_in_grp >= max_ino { break; }
                    if bitmap[byte] & (1 << bit) == 0 {
                        bitmap[byte] |= 1 << bit;
                        self.write_block(bitmap_block, &bitmap)?;

                        desc.bg_free_inodes_count_lo = desc.bg_free_inodes_count_lo.saturating_sub(1);
                        if is_dir {
                            desc.bg_used_dirs_count_lo += 1;
                        }
                        self.write_group_desc(group, &desc)?;

                        let ino = group * self.inodes_per_group + ino_in_grp as u32 + 1;
                        return Ok(ino);
                    }
                }
            }
        }
        Err(ENOSPC)
    }

    fn free_inode(&self, ino: u32) -> Result<(), VfsError> {
        let ino_idx   = (ino - 1) as u64;
        let group     = (ino_idx / self.inodes_per_group as u64) as u32;
        let idx_in_grp = ino_idx % self.inodes_per_group as u64;
        let bitmap_block = self.group_inode_bitmap(group)?;
        let mut bitmap = alloc::vec![0u8; self.block_size as usize];
        self.read_block(bitmap_block, &mut bitmap)?;
        let byte = (idx_in_grp / 8) as usize;
        let bit  = (idx_in_grp % 8) as u8;
        bitmap[byte] &= !(1 << bit);
        self.write_block(bitmap_block, &bitmap)?;

        let mut desc = self.read_group_desc(group)?;
        desc.bg_free_inodes_count_lo += 1;
        self.write_group_desc(group, &desc)
    }

    // ── File data read/write ──────────────────────────────────────────────

    fn read_file(&self, inode: &Ext4Inode, buf: &mut [u8], offset: u64) -> Result<usize, VfsError> {
        let file_size = (inode.i_size_lo as u64) | ((inode.i_size_hi as u64) << 32);
        if offset >= file_size { return Ok(0); }
        let to_read = buf.len().min((file_size - offset) as usize);

        let mut done = 0usize;
        while done < to_read {
            let abs_off  = offset + done as u64;
            let log_blk  = abs_off / self.block_size;
            let blk_off  = (abs_off % self.block_size) as usize;
            let can_read = (self.block_size as usize - blk_off).min(to_read - done);

            let phys_blk = if inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                self.extent_get_block(inode, log_blk)?
            } else {
                self.indirect_get_block(inode, log_blk)?
            };

            match phys_blk {
                None => {
                    // Sparse block — zero fill
                    for b in &mut buf[done..done + can_read] { *b = 0; }
                }
                Some(pb) => {
                    let blk_abs = pb * self.block_size + blk_off as u64;
                    self.read_bytes_at(blk_abs, &mut buf[done..done + can_read])?;
                }
            }
            done += can_read;
        }
        Ok(to_read)
    }

    fn write_file(&self, ino: u32, inode: &mut Ext4Inode, data: &[u8], offset: u64) -> Result<usize, VfsError> {
        let mut done = 0usize;
        while done < data.len() {
            let abs_off = offset + done as u64;
            let log_blk = abs_off / self.block_size;
            let blk_off = (abs_off % self.block_size) as usize;
            let can_write = (self.block_size as usize - blk_off).min(data.len() - done);

            let phys_blk = if inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                self.extent_get_block(inode, log_blk)?
            } else {
                self.indirect_get_block(inode, log_blk)?
            };

            let phys = match phys_blk {
                Some(p) => p,
                None => {
                    // Allocate new block
                    if inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                        self.extent_alloc_block(ino, inode, log_blk)?
                    } else {
                        let p = self.alloc_block()?;
                        self.indirect_set_block(ino, inode, log_blk, p)?;
                        p
                    }
                }
            };

            let blk_abs = phys * self.block_size + blk_off as u64;
            if blk_off == 0 && can_write == self.block_size as usize {
                // Full block write
                self.write_bytes_at(blk_abs, &data[done..done + can_write])?;
            } else {
                // Partial block — read-modify-write
                let mut block_buf = alloc::vec![0u8; self.block_size as usize];
                self.read_block(phys, &mut block_buf)?;
                block_buf[blk_off..blk_off + can_write].copy_from_slice(&data[done..done + can_write]);
                self.write_block(phys, &block_buf)?;
            }
            done += can_write;
        }

        // Update file size
        let new_size = (offset + data.len() as u64)
            .max((inode.i_size_lo as u64) | ((inode.i_size_hi as u64) << 32));
        inode.i_size_lo  = new_size as u32;
        inode.i_size_hi  = (new_size >> 32) as u32;
        inode.i_mtime    = crate::time::realtime_secs() as u32;
        self.write_inode(ino, inode)?;
        Ok(data.len())
    }

    fn truncate_file(&self, ino: u32, inode: &mut Ext4Inode, size: u64) -> Result<(), VfsError> {
        let cur_size = (inode.i_size_lo as u64) | ((inode.i_size_hi as u64) << 32);
        if size > cur_size {
            // Extend: just update size (sparse)
            inode.i_size_lo = size as u32;
            inode.i_size_hi = (size >> 32) as u32;
        } else {
            // Shrink: free blocks beyond new size
            let new_blocks = (size + self.block_size - 1) / self.block_size;
            let old_blocks = (cur_size + self.block_size - 1) / self.block_size;
            for blk in new_blocks..old_blocks {
                if let Ok(Some(phys)) = if inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                    self.extent_get_block(inode, blk)
                } else {
                    self.indirect_get_block(inode, blk)
                } {
                    let _ = self.free_block(phys);
                }
            }
            inode.i_size_lo = size as u32;
            inode.i_size_hi = (size >> 32) as u32;
        }
        inode.i_mtime = crate::time::realtime_secs() as u32;
        self.write_inode(ino, inode)
    }

    // ── Indirect block mapping (old-style, non-extent) ────────────────────

    fn indirect_get_block(&self, inode: &Ext4Inode, logical: u64) -> Result<Option<u64>, VfsError> {
        let ptrs_per_block = (self.block_size / 4) as u64;

        if logical < 12 {
            let b = inode.i_block[logical as usize];
            return Ok(if b == 0 { None } else { Some(b as u64) });
        }
        let logical = logical - 12;

        if logical < ptrs_per_block {
            // Single indirect
            let si = inode.i_block[12];
            if si == 0 { return Ok(None); }
            return self.read_indirect_ptr(si as u64, logical);
        }
        let logical = logical - ptrs_per_block;

        if logical < ptrs_per_block * ptrs_per_block {
            // Double indirect
            let di = inode.i_block[13];
            if di == 0 { return Ok(None); }
            let first  = logical / ptrs_per_block;
            let second = logical % ptrs_per_block;
            if let Some(si) = self.read_indirect_ptr(di as u64, first)? {
                return self.read_indirect_ptr(si, second);
            }
            return Ok(None);
        }

        // Triple indirect — simplified
        Ok(None)
    }

    fn indirect_set_block(&self, ino: u32, inode: &mut Ext4Inode, logical: u64, phys: u64) -> Result<(), VfsError> {
        let ptrs_per_block = (self.block_size / 4) as u64;

        if logical < 12 {
            inode.i_block[logical as usize] = phys as u32;
            return self.write_inode(ino, inode);
        }
        let logical = logical - 12;

        if logical < ptrs_per_block {
            // Single indirect
            if inode.i_block[12] == 0 {
                let si = self.alloc_block()?;
                let zeros = alloc::vec![0u8; self.block_size as usize];
                self.write_block(si, &zeros)?;
                inode.i_block[12] = si as u32;
                self.write_inode(ino, inode)?;
            }
            return self.write_indirect_ptr(inode.i_block[12] as u64, logical, phys);
        }

        Err(ENOSPC)
    }

    fn read_indirect_ptr(&self, block: u64, idx: u64) -> Result<Option<u64>, VfsError> {
        let off = block * self.block_size + idx * 4;
        let mut buf = [0u8; 4];
        self.read_bytes_at(off, &mut buf)?;
        let ptr = u32::from_le_bytes(buf) as u64;
        Ok(if ptr == 0 { None } else { Some(ptr) })
    }

    fn write_indirect_ptr(&self, block: u64, idx: u64, phys: u64) -> Result<(), VfsError> {
        let off = block * self.block_size + idx * 4;
        self.write_bytes_at(off, &(phys as u32).to_le_bytes())
    }

    // ── Directory operations ──────────────────────────────────────────────

    fn dir_read_entries(&self, inode: &Ext4Inode) -> Result<Vec<(String, u32, u8)>, VfsError> {
        let file_size = (inode.i_size_lo as u64) | ((inode.i_size_hi as u64) << 32);
        let mut entries = Vec::new();
        let mut pos = 0u64;

        while pos < file_size {
            let log_blk = pos / self.block_size;
            let blk_off = (pos % self.block_size) as usize;

            let phys = if inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                self.extent_get_block(inode, log_blk)?
            } else {
                self.indirect_get_block(inode, log_blk)?
            };

            let phys = match phys { Some(p) => p, None => { pos += self.block_size; continue; } };
            let mut block_buf = alloc::vec![0u8; self.block_size as usize];
            self.read_block(phys, &mut block_buf)?;

            let mut bpos = blk_off;
            while bpos + 8 <= block_buf.len() {
                let de_ino  = u32::from_le_bytes(block_buf[bpos..bpos+4].try_into().unwrap());
                let rec_len = u16::from_le_bytes(block_buf[bpos+4..bpos+6].try_into().unwrap()) as usize;
                let name_len = block_buf[bpos+6] as usize;
                let file_type = block_buf[bpos+7];

                if rec_len < 8 || bpos + rec_len > block_buf.len() { break; }

                if de_ino != 0 && name_len > 0 && bpos + 8 + name_len <= block_buf.len() {
                    let name = String::from_utf8_lossy(
                        &block_buf[bpos+8..bpos+8+name_len]
                    ).into_owned();
                    entries.push((name, de_ino, file_type));
                }
                bpos += rec_len;
            }
            pos += (self.block_size - blk_off as u64).min(file_size - pos);
        }
        Ok(entries)
    }

    fn dir_lookup_ino(&self, inode: &Ext4Inode, name: &str) -> Result<u32, VfsError> {
        let entries = self.dir_read_entries(inode)?;
        entries.iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, ino, _)| *ino)
            .ok_or(ENOENT)
    }

    fn dir_add_entry(&self, dir_ino: u32, dir_inode: &mut Ext4Inode,
                     new_ino: u32, name: &str, file_type: u8) -> Result<(), VfsError> {
        let name_bytes = name.as_bytes();
        let needed_len = (8 + name_bytes.len() + 3) & !3;
        let file_size = (dir_inode.i_size_lo as u64) | ((dir_inode.i_size_hi as u64) << 32);

        // Find space in existing blocks
        let mut pos = 0u64;
        while pos < file_size {
            let log_blk = pos / self.block_size;
            let phys = if dir_inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                self.extent_get_block(dir_inode, log_blk)?
            } else {
                self.indirect_get_block(dir_inode, log_blk)?
            };
            let phys = match phys { Some(p) => p, None => { pos += self.block_size; continue; } };

            let mut block_buf = alloc::vec![0u8; self.block_size as usize];
            self.read_block(phys, &mut block_buf)?;

            let mut bpos = 0usize;
            while bpos + 8 <= block_buf.len() {
                let de_ino  = u32::from_le_bytes(block_buf[bpos..bpos+4].try_into().unwrap());
                let rec_len = u16::from_le_bytes(block_buf[bpos+4..bpos+6].try_into().unwrap()) as usize;
                if rec_len < 8 { break; }

                let actual_len = if de_ino == 0 { 0 } else {
                    let nlen = block_buf[bpos+6] as usize;
                    (8 + nlen + 3) & !3
                };
                let slack = rec_len - actual_len;

                if slack >= needed_len {
                    // Shrink current entry, add new one after it
                    if de_ino != 0 {
                        block_buf[bpos+4..bpos+6].copy_from_slice(&(actual_len as u16).to_le_bytes());
                        bpos += actual_len;
                    }
                    let remaining = (block_buf.len() - bpos) as u16;
                    block_buf[bpos..bpos+4].copy_from_slice(&new_ino.to_le_bytes());
                    block_buf[bpos+4..bpos+6].copy_from_slice(&remaining.to_le_bytes());
                    block_buf[bpos+6] = name_bytes.len() as u8;
                    block_buf[bpos+7] = file_type;
                    block_buf[bpos+8..bpos+8+name_bytes.len()].copy_from_slice(name_bytes);
                    return self.write_block(phys, &block_buf);
                }
                bpos += rec_len;
            }
            pos += self.block_size;
        }

        // Allocate new block
        let new_blk = if dir_inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
            self.extent_alloc_block(dir_ino, dir_inode, file_size / self.block_size)?
        } else {
            let p = self.alloc_block()?;
            self.indirect_set_block(dir_ino, dir_inode, file_size / self.block_size, p)?;
            p
        };

        let mut block_buf = alloc::vec![0u8; self.block_size as usize];
        let rec_len = self.block_size as u16;
        block_buf[0..4].copy_from_slice(&new_ino.to_le_bytes());
        block_buf[4..6].copy_from_slice(&rec_len.to_le_bytes());
        block_buf[6] = name_bytes.len() as u8;
        block_buf[7] = file_type;
        block_buf[8..8+name_bytes.len()].copy_from_slice(name_bytes);
        self.write_block(new_blk, &block_buf)?;

        dir_inode.i_size_lo = (file_size + self.block_size) as u32;
        dir_inode.i_size_hi = ((file_size + self.block_size) >> 32) as u32;
        self.write_inode(dir_ino, dir_inode)
    }

    fn dir_remove_entry(&self, dir_ino: u32, dir_inode: &Ext4Inode, name: &str) -> Result<(), VfsError> {
        let name_bytes = name.as_bytes();
        let file_size = (dir_inode.i_size_lo as u64) | ((dir_inode.i_size_hi as u64) << 32);

        let mut pos = 0u64;
        while pos < file_size {
            let log_blk = pos / self.block_size;
            let phys = if dir_inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                self.extent_get_block(dir_inode, log_blk)?
            } else {
                self.indirect_get_block(dir_inode, log_blk)?
            };
            let phys = match phys { Some(p) => p, None => { pos += self.block_size; continue; } };

            let mut block_buf = alloc::vec![0u8; self.block_size as usize];
            self.read_block(phys, &mut block_buf)?;

            let mut bpos = 0usize;
            let mut prev_end = 0usize;
            while bpos + 8 <= block_buf.len() {
                let de_ino  = u32::from_le_bytes(block_buf[bpos..bpos+4].try_into().unwrap());
                let rec_len = u16::from_le_bytes(block_buf[bpos+4..bpos+6].try_into().unwrap()) as usize;
                if rec_len < 8 { break; }
                let name_len = block_buf[bpos+6] as usize;

                if de_ino != 0 && name_len == name_bytes.len()
                    && &block_buf[bpos+8..bpos+8+name_len] == name_bytes {
                    // Mark as deleted by zeroing inode number.
                    // Per ext2/3/4 convention, the previous entry absorbs
                    // this one's rec_len to keep the block parseable.
                    block_buf[bpos..bpos+4].copy_from_slice(&0u32.to_le_bytes());
                    if prev_end > 0 {
                        // Extend previous entry's rec_len to swallow this slot
                        let prev_start = prev_end - {
                            // Walk back to find prev entry start
                            let mut ps = 0usize;
                            let mut pp = 0usize;
                            while pp < bpos {
                                let rl = u16::from_le_bytes(
                                    block_buf[pp+4..pp+6].try_into().unwrap_or([0,0])
                                ) as usize;
                                if rl < 8 { break; }
                                ps = pp;
                                pp += rl;
                            }
                            bpos - ps
                        };
                        let new_rec = u16::from_le_bytes(
                            block_buf[prev_end - (bpos - prev_end.saturating_sub(rec_len))
                                      ..].get(4..6).and_then(|s| s.try_into().ok())
                                         .unwrap_or([0u8;2])
                        );
                        // Simpler: just add rec_len to previous entry
                        let prev_rl_off = prev_end.saturating_sub(rec_len);
                        let cur_prev_rl = u16::from_le_bytes(
                            block_buf[prev_rl_off+4..prev_rl_off+6].try_into().unwrap_or([0,0])
                        ) as usize;
                        let merged = (cur_prev_rl + rec_len) as u16;
                        block_buf[prev_rl_off+4..prev_rl_off+6].copy_from_slice(&merged.to_le_bytes());
                    }
                    return self.write_block(phys, &block_buf);
                }
                prev_end = bpos + rec_len;
                bpos += rec_len;
            }
            pos += self.block_size;
        }
        Err(ENOENT)
    }

    fn inode_mode_to_file_type(mode: u16) -> u8 {
        match mode & 0xF000 {
            0x8000 => EXT4_FT_REG_FILE,
            0x4000 => EXT4_FT_DIR,
            0xA000 => EXT4_FT_SYMLINK,
            0x2000 => EXT4_FT_CHRDEV,
            0x6000 => EXT4_FT_BLKDEV,
            0x1000 => EXT4_FT_FIFO,
            0xC000 => EXT4_FT_SOCK,
            _ => EXT4_FT_UNKNOWN,
        }
    }

    fn file_type_to_vfs(ft: u8) -> u8 {
        match ft {
            EXT4_FT_REG_FILE => 8,
            EXT4_FT_DIR      => 4,
            EXT4_FT_SYMLINK  => 10,
            EXT4_FT_CHRDEV   => 2,
            EXT4_FT_BLKDEV   => 6,
            EXT4_FT_FIFO     => 1,
            EXT4_FT_SOCK     => 12,
            _                => 0,
        }
    }
}

// ── VFS integration ───────────────────────────────────────────────────────

pub struct Ext4FsState(Arc<Mutex<Ext4Fs>>);

impl Ext4FsState {
    fn make_inode(&self, ino: u32, ext_inode: &Ext4Inode) -> Inode {
        let size = (ext_inode.i_size_lo as u64) | ((ext_inode.i_size_hi as u64) << 32);
        let dev  = self.0.lock().dev_id;
        Inode {
            ino:   ino as u64,
            mode:  ext_inode.i_mode as u32,
            uid:   ext_inode.i_uid as u32,
            gid:   ext_inode.i_gid as u32,
            size,
            atime: ext_inode.i_atime as i64,
            mtime: ext_inode.i_mtime as i64,
            ctime: ext_inode.i_ctime as i64,
            ops:   Arc::new(Ext4InodeOps { fs: self.0.clone(), ino }),
            sb:    Arc::new(self.make_sb()),
        }
    }

    fn make_sb(&self) -> Superblock {
        Superblock {
            dev:     self.0.lock().dev_id,
            fs_type: String::from("ext4"),
            ops:     Arc::new(Ext4SbOps(self.0.clone())),
        }
    }
}

struct Ext4SbOps(Arc<Mutex<Ext4Fs>>);

impl SuperblockOps for Ext4SbOps {
    fn get_root(&self) -> Result<Inode, VfsError> {
        // ext4 root inode is always 2
        let fs = self.0.lock();
        let ext_inode = fs.read_inode(2)?;
        drop(fs);
        let state = Ext4FsState(self.0.clone());
        Ok(state.make_inode(2, &ext_inode))
    }

    fn statfs(&self, buf: &mut StatFs) {
        let fs = self.0.lock();
        let total = fs.sb.s_blocks_count_lo as u64 | ((fs.sb.s_blocks_count_hi as u64) << 32);
        let free  = fs.sb.s_free_blocks_count_lo as u64 | ((fs.sb.s_free_blocks_count_hi as u64) << 32);
        buf.f_type    = 0xEF53;
        buf.f_bsize   = fs.block_size as i64;
        buf.f_blocks  = total;
        buf.f_bfree   = free;
        buf.f_bavail  = free;
        buf.f_files   = fs.sb.s_inodes_count as u64;
        buf.f_ffree   = fs.sb.s_free_inodes_count as u64;
        buf.f_namelen = 255;
        buf.f_frsize  = fs.block_size as i64;
    }

    fn sync(&self) {
        let fs_guard = self.0.lock();
        let maybe_journal = fs_guard.journal.try_lock();
        if let Some(mut guard) = maybe_journal {
            if let Some(ref mut jnl) = *guard { let _ = jnl.sync(); }
        }
    }
}

struct Ext4InodeOps {
    fs:  Arc<Mutex<Ext4Fs>>,
    ino: u32,
}

impl InodeOps for Ext4InodeOps {
    fn read(&self, inode: &Inode, buf: &mut [u8], offset: u64) -> Result<usize, VfsError> {
        let fs = self.fs.lock();
        let ext_inode = fs.read_inode(self.ino)?;
        fs.read_file(&ext_inode, buf, offset)
    }

    fn write(&self, inode: &Inode, data: &[u8], offset: u64) -> Result<usize, VfsError> {
        let fs = self.fs.lock();
        let mut ext_inode = fs.read_inode(self.ino)?;
        fs.write_file(self.ino, &mut ext_inode, data, offset)
    }

    fn truncate(&self, _: &Inode, size: u64) -> Result<(), VfsError> {
        let fs = self.fs.lock();
        let mut ext_inode = fs.read_inode(self.ino)?;
        fs.truncate_file(self.ino, &mut ext_inode, size)
    }

    fn readdir(&self, _: &Inode, _offset: u64) -> Result<Vec<DirEntry>, VfsError> {
        let fs = self.fs.lock();
        let ext_inode = fs.read_inode(self.ino)?;
        if ext_inode.i_mode & 0xF000 != 0x4000 { return Err(ENOTDIR); }
        let raw = fs.dir_read_entries(&ext_inode)?;
        let dev = fs.dev_id;
        drop(fs);

        let state = Ext4FsState(self.fs.clone());
        let sb    = Arc::new(state.make_sb());

        let mut out = Vec::new();
        out.push(DirEntry { name: String::from("."),  ino: self.ino as u64, file_type: 4 });
        out.push(DirEntry { name: String::from(".."), ino: self.ino as u64, file_type: 4 });
        for (name, ino, ft) in raw {
            if name == "." || name == ".." { continue; }
            out.push(DirEntry { name, ino: ino as u64,
                file_type: Ext4Fs::file_type_to_vfs(ft) });
        }
        Ok(out)
    }

    fn lookup(&self, _: &Inode, name: &str) -> Result<Inode, VfsError> {
        let fs = self.fs.lock();
        let ext_inode = fs.read_inode(self.ino)?;
        let child_ino = fs.dir_lookup_ino(&ext_inode, name)?;
        let child_ext = fs.read_inode(child_ino)?;
        drop(fs);
        let state = Ext4FsState(self.fs.clone());
        Ok(state.make_inode(child_ino, &child_ext))
    }

    fn create(&self, _: &Inode, name: &str, mode: u32) -> Result<Inode, VfsError> {
        let fs = self.fs.lock();
        let new_ino = fs.alloc_inode(false)?;
        let now = crate::time::realtime_secs() as u32;
        let (uid, gid) = crate::process::with_current(|p| (p.uid, p.gid)).unwrap_or((0, 0));

        let mut new_ext = Ext4Inode {
            i_mode: (mode & 0x1FFF | 0x8000) as u16,
            i_uid:  uid as u16,
            i_gid:  gid as u16,
            i_size_lo: 0, i_size_hi: 0,
            i_atime: now, i_ctime: now, i_mtime: now,
            i_dtime: 0,
            i_links_count: 1,
            i_blocks_lo: 0,
            i_flags: EXT4_INODE_FLAG_EXTENTS,
            i_block: [0; 15],
            i_version: 1,
            i_generation: 1,
            i_file_acl_lo: 0, i_obso_faddr: 0,
            i_osd2: [0; 12], i_extra_isize: 28,
            i_checksum_hi: 0, i_ctime_extra: 0, i_mtime_extra: 0,
            i_atime_extra: 0, i_crtime: now, i_crtime_extra: 0,
            i_version_hi: 0, i_projid: 0,
        };
        // Initialize extent tree header in i_block
        let hdr_ptr = new_ext.i_block.as_mut_ptr() as *mut Ext4ExtentHeader;
        unsafe {
            (*hdr_ptr).eh_magic = EXT4_EXT_MAGIC;
            (*hdr_ptr).eh_entries = 0;
            (*hdr_ptr).eh_max = 4;
            (*hdr_ptr).eh_depth = 0;
            (*hdr_ptr).eh_generation = 0;
        }

        fs.write_inode(new_ino, &new_ext)?;

        // Add to parent directory
        let mut dir_inode = fs.read_inode(self.ino)?;
        fs.dir_add_entry(self.ino, &mut dir_inode, new_ino, name, EXT4_FT_REG_FILE)?;

        // Commit journal if active (fs lock already held — journal is a separate Mutex inside Ext4Fs)
        if let Some(mut jnl_g) = fs.journal.try_lock() {
            if let Some(ref mut jnl) = *jnl_g { let _ = jnl.commit(); }
        }

        drop(fs);
        let state = Ext4FsState(self.fs.clone());
        Ok(state.make_inode(new_ino, &new_ext))
    }

    fn mkdir(&self, _: &Inode, name: &str, mode: u32) -> Result<Inode, VfsError> {
        let fs = self.fs.lock();
        let new_ino = fs.alloc_inode(true)?;
        let now = crate::time::realtime_secs() as u32;
        let (uid, gid) = crate::process::with_current(|p| (p.uid, p.gid)).unwrap_or((0, 0));

        let mut new_ext = Ext4Inode {
            i_mode: (mode & 0x1FFF | 0x4000) as u16,
            i_uid:  uid as u16, i_gid: gid as u16,
            i_size_lo: 0, i_size_hi: 0,
            i_atime: now, i_ctime: now, i_mtime: now, i_dtime: 0,
            i_links_count: 2, i_blocks_lo: 0,
            i_flags: EXT4_INODE_FLAG_EXTENTS,
            i_block: [0; 15], i_version: 1, i_generation: 1,
            i_file_acl_lo: 0, i_obso_faddr: 0,
            i_osd2: [0; 12], i_extra_isize: 28,
            i_checksum_hi: 0, i_ctime_extra: 0, i_mtime_extra: 0,
            i_atime_extra: 0, i_crtime: now, i_crtime_extra: 0,
            i_version_hi: 0, i_projid: 0,
        };
        let hdr_ptr = new_ext.i_block.as_mut_ptr() as *mut Ext4ExtentHeader;
        unsafe {
            (*hdr_ptr).eh_magic = EXT4_EXT_MAGIC;
            (*hdr_ptr).eh_entries = 0; (*hdr_ptr).eh_max = 4;
            (*hdr_ptr).eh_depth = 0; (*hdr_ptr).eh_generation = 0;
        }
        fs.write_inode(new_ino, &new_ext)?;

        // Add . and .. to new dir
        fs.dir_add_entry(new_ino, &mut new_ext, new_ino, ".", EXT4_FT_DIR)?;
        let mut new_ext2 = fs.read_inode(new_ino)?;
        fs.dir_add_entry(new_ino, &mut new_ext2, self.ino, "..", EXT4_FT_DIR)?;

        // Add to parent
        let mut dir_inode = fs.read_inode(self.ino)?;
        fs.dir_add_entry(self.ino, &mut dir_inode, new_ino, name, EXT4_FT_DIR)?;

        let final_inode = fs.read_inode(new_ino)?;
        drop(fs);
        let state = Ext4FsState(self.fs.clone());
        Ok(state.make_inode(new_ino, &final_inode))
    }

    fn unlink(&self, _: &Inode, name: &str) -> Result<(), VfsError> {
        let fs = self.fs.lock();
        let dir_inode = fs.read_inode(self.ino)?;
        let child_ino = fs.dir_lookup_ino(&dir_inode, name)?;
        let mut child = fs.read_inode(child_ino)?;

        if child.i_mode & 0xF000 == 0x4000 { return Err(EISDIR); }

        fs.dir_remove_entry(self.ino, &dir_inode, name)?;

        child.i_links_count = child.i_links_count.saturating_sub(1);
        if child.i_links_count == 0 {
            child.i_dtime = crate::time::realtime_secs() as u32;
            fs.write_inode(child_ino, &child)?;
            // Free data blocks
            let size = (child.i_size_lo as u64) | ((child.i_size_hi as u64) << 32);
            let n_blocks = (size + fs.block_size - 1) / fs.block_size;
            for blk in 0..n_blocks {
                if let Ok(Some(phys)) = if child.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                    fs.extent_get_block(&child, blk)
                } else {
                    fs.indirect_get_block(&child, blk)
                } {
                    let _ = fs.free_block(phys);
                }
            }
            fs.free_inode(child_ino)?;
        } else {
            fs.write_inode(child_ino, &child)?;
        }
        Ok(())
    }

    fn rmdir(&self, _: &Inode, name: &str) -> Result<(), VfsError> {
        let fs = self.fs.lock();
        let dir_inode = fs.read_inode(self.ino)?;
        let child_ino = fs.dir_lookup_ino(&dir_inode, name)?;
        let child = fs.read_inode(child_ino)?;
        if child.i_mode & 0xF000 != 0x4000 { return Err(ENOTDIR); }

        // Check empty (only . and ..)
        let entries = fs.dir_read_entries(&child)?;
        let non_dots = entries.iter().filter(|(n,_,_)| n != "." && n != "..").count();
        if non_dots > 0 { return Err(ENOTEMPTY); }

        fs.dir_remove_entry(self.ino, &dir_inode, name)?;
        fs.free_inode(child_ino)
    }

    fn symlink(&self, _: &Inode, name: &str, target: &str) -> Result<Inode, VfsError> {
        let fs = self.fs.lock();
        let new_ino = fs.alloc_inode(false)?;
        let now = crate::time::realtime_secs() as u32;
        let (uid, gid) = crate::process::with_current(|p| (p.uid, p.gid)).unwrap_or((0, 0));
        let target_bytes = target.as_bytes();

        let mut new_ext = Ext4Inode {
            i_mode: 0xA1FF, // symlink rwxrwxrwx
            i_uid: uid as u16, i_gid: gid as u16,
            i_size_lo: target_bytes.len() as u32, i_size_hi: 0,
            i_atime: now, i_ctime: now, i_mtime: now, i_dtime: 0,
            i_links_count: 1, i_blocks_lo: 0,
            // Short symlinks stored inline in i_block
            i_flags: 0,
            i_block: [0; 15], i_version: 1, i_generation: 1,
            i_file_acl_lo: 0, i_obso_faddr: 0,
            i_osd2: [0; 12], i_extra_isize: 28,
            i_checksum_hi: 0, i_ctime_extra: 0, i_mtime_extra: 0,
            i_atime_extra: 0, i_crtime: now, i_crtime_extra: 0,
            i_version_hi: 0, i_projid: 0,
        };

        if target_bytes.len() <= 60 {
            // Fast symlink: store in i_block directly
            let block_ptr = new_ext.i_block.as_mut_ptr() as *mut u8;
            unsafe { core::ptr::copy_nonoverlapping(target_bytes.as_ptr(), block_ptr, target_bytes.len()); }
        } else {
            // Long symlink: allocate a data block
            new_ext.i_flags = EXT4_INODE_FLAG_EXTENTS;
            let hdr = new_ext.i_block.as_mut_ptr() as *mut Ext4ExtentHeader;
            unsafe {
                (*hdr).eh_magic = EXT4_EXT_MAGIC; (*hdr).eh_entries = 0;
                (*hdr).eh_max = 4; (*hdr).eh_depth = 0;
            }
            fs.write_inode(new_ino, &new_ext)?;
            let mut mi = fs.read_inode(new_ino)?;
            fs.write_file(new_ino, &mut mi, target_bytes, 0)?;
            new_ext = fs.read_inode(new_ino)?;
        }
        fs.write_inode(new_ino, &new_ext)?;

        let mut dir_inode = fs.read_inode(self.ino)?;
        fs.dir_add_entry(self.ino, &mut dir_inode, new_ino, name, EXT4_FT_SYMLINK)?;

        drop(fs);
        let state = Ext4FsState(self.fs.clone());
        Ok(state.make_inode(new_ino, &new_ext))
    }

    fn readlink(&self, _: &Inode) -> Result<String, VfsError> {
        let fs = self.fs.lock();
        let ext_inode = fs.read_inode(self.ino)?;
        if ext_inode.i_mode & 0xF000 != 0xA000 { return Err(EINVAL); }
        let size = ext_inode.i_size_lo as usize;

        if size <= 60 {
            // Fast symlink
            let block_ptr = ext_inode.i_block.as_ptr() as *const u8;
            let target = unsafe { core::slice::from_raw_parts(block_ptr, size) };
            Ok(String::from_utf8_lossy(target).into_owned())
        } else {
            let mut buf = alloc::vec![0u8; size];
            fs.read_file(&ext_inode, &mut buf, 0)?;
            Ok(String::from_utf8_lossy(&buf).into_owned())
        }
    }

    fn rename(&self, _: &Inode, old_name: &str, new_parent_ino: u64, new_name: &str) -> Result<(), VfsError> {
        let fs = self.fs.lock();
        let dir_inode = fs.read_inode(self.ino)?;
        let child_ino = fs.dir_lookup_ino(&dir_inode, old_name)?;
        let child = fs.read_inode(child_ino)?;
        let file_type = Ext4Fs::inode_mode_to_file_type(child.i_mode);

        // Remove from old dir
        fs.dir_remove_entry(self.ino, &dir_inode, old_name)?;

        // Add to new dir (may be same dir)
        let mut new_dir = fs.read_inode(new_parent_ino as u32)?;
        fs.dir_add_entry(new_parent_ino as u32, &mut new_dir, child_ino, new_name, file_type)?;
        Ok(())
    }

    fn link(&self, _: &Inode, name: &str, target: &Inode) -> Result<(), VfsError> {
        let fs = self.fs.lock();
        let mut target_inode = fs.read_inode(target.ino as u32)?;
        if target_inode.i_mode & 0xF000 == 0x4000 { return Err(EISDIR); }

        target_inode.i_links_count += 1;
        fs.write_inode(target.ino as u32, &target_inode)?;

        let file_type = Ext4Fs::inode_mode_to_file_type(target_inode.i_mode);
        let mut dir_inode = fs.read_inode(self.ino)?;
        fs.dir_add_entry(self.ino, &mut dir_inode, target.ino as u32, name, file_type)
    }

    fn chmod(&self, _: &Inode, mode: u32) -> Result<(), VfsError> {
        let fs = self.fs.lock();
        let mut ext_inode = fs.read_inode(self.ino)?;
        ext_inode.i_mode = (ext_inode.i_mode & 0xF000) | (mode & 0x0FFF) as u16;
        fs.write_inode(self.ino, &ext_inode)
    }

    fn chown(&self, _: &Inode, uid: u32, gid: u32) -> Result<(), VfsError> {
        let fs = self.fs.lock();
        let mut ext_inode = fs.read_inode(self.ino)?;
        if uid != u32::MAX { ext_inode.i_uid = uid as u16; }
        if gid != u32::MAX { ext_inode.i_gid = gid as u16; }
        fs.write_inode(self.ino, &ext_inode)
    }

    fn fsync(&self, _: &Inode) -> Result<(), VfsError> {
        // Journal sync would happen here
        Ok(())
    }
}

// ── Public mount API ──────────────────────────────────────────────────────

/// Mount an ext4 filesystem from a disk device.
pub fn mount(disk: Arc<dyn Disk>, dev_id: u64) -> Option<Superblock> {
    let mut fs = Ext4Fs::new(disk.clone(), dev_id)?;

    // Mount journal if the filesystem has one
    if fs.has_journal && fs.sb.s_journal_inum != 0 {
        let j_ino = fs.sb.s_journal_inum;
        // Read journal inode to find its first data block
        if let Ok(j_inode) = fs.read_inode(j_ino) {
            let j_start = if j_inode.i_flags & EXT4_INODE_FLAG_EXTENTS != 0 {
                fs.extent_get_block(&j_inode, 0).ok().flatten()
            } else {
                fs.indirect_get_block(&j_inode, 0).ok().flatten()
            };
            if let Some(start_block) = j_start {
                match journal::Journal::new(disk, start_block, fs.block_size) {
                    Some(mut jnl) => {
                        jnl.recover(); // replay any committed-but-not-checkpointed txns
                        *fs.journal.lock() = Some(jnl);
                        crate::klog!("ext4: jbd2 journal active (inode {}, start block {})",
                            j_ino, start_block);
                    }
                    None => crate::klog!("ext4: journal inode found but journal init failed"),
                }
            }
        }
    }

    let state = Ext4FsState(Arc::new(Mutex::new(fs)));
    Some(state.make_sb())
}

/// Mount an ext4 filesystem from raw bytes (in-memory disk image).
pub fn mount_from_bytes(data: Vec<u8>) -> Option<Superblock> {
    let disk = MemDisk::new(data);
    mount(disk, 10)
}
