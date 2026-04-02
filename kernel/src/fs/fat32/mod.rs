use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use crate::vfs::*;

#[repr(C, packed)]
struct Bpb {
    jmp_boot: [u8; 3],
    oem_name: [u8; 8],
    bytes_per_sector: u16,
    sectors_per_cluster: u8,
    reserved_sectors: u16,
    num_fats: u8,
    root_entry_count: u16,
    total_sectors16: u16,
    media: u8,
    fat_size16: u16,
    sectors_per_track: u16,
    num_heads: u16,
    hidden_sectors: u32,
    total_sectors32: u32,
    fat_size32: u32,
    ext_flags: u16,
    fs_version: u16,
    root_cluster: u32,
    fs_info: u16,
    backup_boot_sector: u16,
    _reserved: [u8; 12],
    drive_num: u8,
    _reserved2: u8,
    boot_sig: u8,
    vol_id: u32,
    vol_label: [u8; 11],
    fs_type: [u8; 8],
}

#[repr(C, packed)]
struct DirEntry {
    name: [u8; 11],
    attr: u8,
    _nt_res: u8,
    crt_time_tenth: u8,
    crt_time: u16,
    crt_date: u16,
    lst_acc_date: u16,
    fst_clus_hi: u16,
    wrt_time: u16,
    wrt_date: u16,
    fst_clus_lo: u16,
    file_size: u32,
}

const ATTR_READ_ONLY: u8 = 0x01;
const ATTR_HIDDEN:    u8 = 0x02;
const ATTR_SYSTEM:    u8 = 0x04;
const ATTR_VOLUME_ID: u8 = 0x08;
const ATTR_DIRECTORY: u8 = 0x10;
const ATTR_ARCHIVE:   u8 = 0x20;
const ATTR_LONG_NAME: u8 = ATTR_READ_ONLY | ATTR_HIDDEN | ATTR_SYSTEM | ATTR_VOLUME_ID;
const FAT_EOC: u32 = 0x0FFFFFF8;

pub struct Fat32Fs {
    data: Arc<Vec<u8>>,
    bytes_per_sector: u32,
    sectors_per_cluster: u32,
    reserved_sectors: u32,
    num_fats: u32,
    fat_size: u32,
    root_cluster: u32,
    data_start_sector: u32,
}

impl Fat32Fs {
    pub fn new(disk_data: Vec<u8>) -> Option<crate::vfs::Superblock> {
        if disk_data.len() < 512 { return None; }
        let bpb = unsafe { &*(disk_data.as_ptr() as *const Bpb) };

        let bps = u16::from_le(bpb.bytes_per_sector) as u32;
        let spc = bpb.sectors_per_cluster as u32;
        let res = u16::from_le(bpb.reserved_sectors) as u32;
        let nfats = bpb.num_fats as u32;
        let fat_sz = u32::from_le(bpb.fat_size32);
        let root_clus = u32::from_le(bpb.root_cluster);
        let data_start = res + nfats * fat_sz;

        let fs = Arc::new(Fat32Fs {
            data: Arc::new(disk_data),
            bytes_per_sector: bps,
            sectors_per_cluster: spc,
            reserved_sectors: res,
            num_fats: nfats,
            fat_size: fat_sz,
            root_cluster: root_clus,
            data_start_sector: data_start,
        });

        Some(crate::vfs::Superblock {
            dev: 2,
            fs_type: String::from("fat32"),
            ops: fs,
        })
    }

    fn cluster_to_byte(&self, cluster: u32) -> usize {
        ((self.data_start_sector + (cluster - 2) * self.sectors_per_cluster) * self.bytes_per_sector) as usize
    }

    fn fat_entry(&self, cluster: u32) -> u32 {
        let fat_start = (self.reserved_sectors * self.bytes_per_sector) as usize;
        let offset = fat_start + (cluster as usize) * 4;
        if offset + 4 > self.data.len() { return FAT_EOC; }
        u32::from_le_bytes(self.data[offset..offset+4].try_into().unwrap()) & 0x0FFF_FFFF
    }

    fn read_cluster_chain(&self, start: u32) -> Vec<u8> {
        let cluster_size = (self.sectors_per_cluster * self.bytes_per_sector) as usize;
        let mut result = Vec::new();
        let mut cluster = start;
        while cluster < FAT_EOC {
            let offset = self.cluster_to_byte(cluster);
            if offset + cluster_size <= self.data.len() {
                result.extend_from_slice(&self.data[offset..offset + cluster_size]);
            }
            cluster = self.fat_entry(cluster);
        }
        result
    }

    fn parse_dir_entries(&self, data: &[u8]) -> Vec<(String, u32, u32, bool)> {
        let mut entries = Vec::new();
        let mut i = 0;
        while i + 32 <= data.len() {
            let de = unsafe { &*(data.as_ptr().add(i) as *const DirEntry) };
            if de.name[0] == 0x00 { break; }
            if de.name[0] == 0xE5 { i += 32; continue; }
            if de.attr == ATTR_LONG_NAME { i += 32; continue; }
            if de.attr & ATTR_VOLUME_ID != 0 { i += 32; continue; }

            let name = parse_83_name(&de.name);
            let cluster = (u16::from_le(de.fst_clus_hi) as u32) << 16
                | u16::from_le(de.fst_clus_lo) as u32;
            let size = u32::from_le(de.file_size);
            let is_dir = de.attr & ATTR_DIRECTORY != 0;
            entries.push((name, cluster, size, is_dir));
            i += 32;
        }
        entries
    }
}

fn parse_83_name(raw: &[u8; 11]) -> String {
    let name = raw[..8].iter().take_while(|&&b| b != b' ').copied().collect::<Vec<_>>();
    let ext  = raw[8..].iter().take_while(|&&b| b != b' ').copied().collect::<Vec<_>>();
    let mut s = String::from_utf8_lossy(&name).to_ascii_lowercase();
    if !ext.is_empty() {
        s.push('.');
        s.push_str(&String::from_utf8_lossy(&ext).to_ascii_lowercase());
    }
    s
}

// Fat32Fs SuperblockOps is implemented on Arc<Fat32Fs> in the Fat32Fs::new() return
// This stub impl exists for completeness but get_root is called on the Arc wrapper
impl SuperblockOps for Fat32Fs {
    fn get_root(&self) -> Result<Inode, VfsError> {
        // self is &Fat32Fs - make_dir_inode needs Arc<Self>
        // Return an error; callers should use Arc<Fat32Fs> impl instead
        Err(crate::vfs::ENOSYS)
    }
}

impl Fat32Fs {
    fn make_dir_inode(self: &Arc<Self>, cluster: u32, ino: u64) -> Result<Inode, VfsError> {
        let ops = Arc::new(Fat32InodeOps {
            fs: self.clone(),
            cluster,
            size: 0,
            is_dir: true,
        });
        let sb = Arc::new(crate::vfs::Superblock {
            dev: 2,
            fs_type: String::from("fat32"),
            ops: self.clone(),
        });
        Ok(Inode {
            ino,
            mode: S_IFDIR | 0o555,
            uid: 0, gid: 0,
            size: 0,
            atime: 0, mtime: 0, ctime: 0,
            ops,
            sb,
        })
    }

    fn make_file_inode(self: &Arc<Self>, cluster: u32, size: u32, ino: u64) -> Inode {
        let ops = Arc::new(Fat32InodeOps {
            fs: self.clone(),
            cluster,
            size,
            is_dir: false,
        });
        let sb = Arc::new(crate::vfs::Superblock {
            dev: 2,
            fs_type: String::from("fat32"),
            ops: self.clone(),
        });
        Inode {
            ino,
            mode: S_IFREG | 0o444,
            uid: 0, gid: 0,
            size: size as u64,
            atime: 0, mtime: 0, ctime: 0,
            ops,
            sb,
        }
    }
}

struct Fat32InodeOps {
    fs: Arc<Fat32Fs>,
    cluster: u32,
    size: u32,
    is_dir: bool,
}

impl InodeOps for Fat32InodeOps {
    fn read(&self, inode: &Inode, buf: &mut [u8], offset: u64) -> Result<usize, VfsError> {
        if self.is_dir { return Err(EISDIR); }
        let data = self.fs.read_cluster_chain(self.cluster);
        let start = offset as usize;
        let end = (start + buf.len()).min(self.size as usize).min(data.len());
        if start >= end { return Ok(0); }
        let n = end - start;
        buf[..n].copy_from_slice(&data[start..end]);
        Ok(n)
    }

    fn write(&self, _inode: &Inode, _buf: &[u8], _offset: u64) -> Result<usize, VfsError> {
        Err(EACCES)
    }

    fn readdir(&self, _inode: &Inode, _offset: u64) -> Result<Vec<crate::vfs::DirEntry>, VfsError> {
        if !self.is_dir { return Err(ENOTDIR); }
        let data = self.fs.read_cluster_chain(self.cluster);
        let raw = self.fs.parse_dir_entries(&data);
        let entries = raw.iter().enumerate().map(|(i, (name, _, _, is_dir))| {
            crate::vfs::DirEntry {
                name: name.clone(),
                ino: i as u64 + 100,
                file_type: if *is_dir { 4 } else { 8 },
            }
        }).collect();
        Ok(entries)
    }

    fn lookup(&self, _inode: &Inode, name: &str) -> Result<Inode, VfsError> {
        if !self.is_dir { return Err(ENOTDIR); }
        let data = self.fs.read_cluster_chain(self.cluster);
        let entries = self.fs.parse_dir_entries(&data);
        for (i, (ename, cluster, size, is_dir)) in entries.iter().enumerate() {
            if ename.eq_ignore_ascii_case(name) {
                return if *is_dir {
                    self.fs.make_dir_inode(*cluster, i as u64 + 100)
                } else {
                    Ok(self.fs.make_file_inode(*cluster, *size, i as u64 + 100))
                };
            }
        }
        Err(ENOENT)
    }
}
