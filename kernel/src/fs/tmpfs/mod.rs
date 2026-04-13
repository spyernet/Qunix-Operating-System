//! tmpfs - memory-backed filesystem, full read/write/create/mkdir/unlink.
//!
//! File contents are stored in 4 KiB pages so boot-time copies of larger ELF
//! binaries do not depend on one large contiguous heap allocation.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

use crate::arch::x86_64::paging::phys_to_virt;
use crate::memory::phys::{alloc_frame, free_frame};
use crate::vfs::*;

const TMPFS_PAGE_SIZE: usize = 4096;

struct TmpFileData {
    len: usize,
    pages: Vec<u64>,
}

impl TmpFileData {
    fn new() -> Self {
        Self { len: 0, pages: Vec::new() }
    }

    fn len(&self) -> usize {
        self.len
    }

    fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn required_pages(len: usize) -> usize {
        if len == 0 {
            0
        } else {
            (len + TMPFS_PAGE_SIZE - 1) / TMPFS_PAGE_SIZE
        }
    }

    fn ensure_len(&mut self, len: usize) -> Result<(), VfsError> {
        let needed = Self::required_pages(len);
        while self.pages.len() < needed {
            let frame = alloc_frame().ok_or(ENOMEM)?;
            unsafe {
                core::ptr::write_bytes(phys_to_virt(frame) as *mut u8, 0, TMPFS_PAGE_SIZE);
            }
            self.pages.push(frame);
            let count = self.pages.len();
            if count % 8 == 0 || count == needed {
                crate::klog!("tmpfs_file: allocated page {}/{}", count, needed);
            }
        }
        Ok(())
    }

    fn page_slice(frame: u64) -> &'static [u8] {
        unsafe { core::slice::from_raw_parts(phys_to_virt(frame) as *const u8, TMPFS_PAGE_SIZE) }
    }

    fn page_slice_mut(frame: u64) -> &'static mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(phys_to_virt(frame) as *mut u8, TMPFS_PAGE_SIZE) }
    }

    fn zero_range(&mut self, start: usize, end: usize) -> Result<(), VfsError> {
        if end <= start {
            return Ok(());
        }

        self.ensure_len(end)?;

        let mut pos = start;
        while pos < end {
            let page_idx = pos / TMPFS_PAGE_SIZE;
            let page_off = pos % TMPFS_PAGE_SIZE;
            let next = end.min((page_idx + 1) * TMPFS_PAGE_SIZE);
            Self::page_slice_mut(self.pages[page_idx])[page_off..page_off + (next - pos)].fill(0);
            pos = next;
        }
        Ok(())
    }

    fn read(&self, offset: usize, buf: &mut [u8]) -> usize {
        if offset >= self.len {
            return 0;
        }

        let total = buf.len().min(self.len - offset);
        let mut copied = 0usize;
        while copied < total {
            let pos = offset + copied;
            let page_idx = pos / TMPFS_PAGE_SIZE;
            let page_off = pos % TMPFS_PAGE_SIZE;
            let chunk = (total - copied).min(TMPFS_PAGE_SIZE - page_off);
            buf[copied..copied + chunk]
                .copy_from_slice(&Self::page_slice(self.pages[page_idx])[page_off..page_off + chunk]);
            copied += chunk;
        }

        copied
    }

    fn write(&mut self, offset: usize, data: &[u8]) -> Result<usize, VfsError> {
        if data.is_empty() {
            return Ok(0);
        }

        let end = offset + data.len();
        if offset > self.len {
            self.zero_range(self.len, offset)?;
        }
        self.ensure_len(end)?;

        let mut copied = 0usize;
        while copied < data.len() {
            let pos = offset + copied;
            let page_idx = pos / TMPFS_PAGE_SIZE;
            let page_off = pos % TMPFS_PAGE_SIZE;
            let chunk = (data.len() - copied).min(TMPFS_PAGE_SIZE - page_off);
            Self::page_slice_mut(self.pages[page_idx])[page_off..page_off + chunk]
                .copy_from_slice(&data[copied..copied + chunk]);
            copied += chunk;
            if copied % (64 * 1024) == 0 || copied == data.len() {
                crate::klog!("tmpfs_file: copied {}/{} bytes", copied, data.len());
            }
        }

        if end > self.len {
            self.len = end;
        }

        Ok(copied)
    }

    fn truncate(&mut self, len: usize) -> Result<(), VfsError> {
        if len > self.len {
            self.zero_range(self.len, len)?;
            self.len = len;
            return Ok(());
        }

        self.len = len;
        let needed = Self::required_pages(len);
        while self.pages.len() > needed {
            if let Some(frame) = self.pages.pop() {
                free_frame(frame);
            }
        }

        if needed > 0 && len % TMPFS_PAGE_SIZE != 0 {
            let tail = len % TMPFS_PAGE_SIZE;
            Self::page_slice_mut(self.pages[needed - 1])[tail..].fill(0);
        }
        Ok(())
    }
}

impl Drop for TmpFileData {
    fn drop(&mut self) {
        while let Some(frame) = self.pages.pop() {
            free_frame(frame);
        }
    }
}

enum NodeData {
    File(TmpFileData),
    Dir(BTreeMap<String, u64>),
    Symlink(String),
}

struct TmpNode {
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    data: NodeData,
    atime: i64,
    mtime: i64,
    ctime: i64,
    nlink: u32,
}

impl TmpNode {
    fn size(&self) -> u64 {
        match &self.data {
            NodeData::File(f) => f.len() as u64,
            NodeData::Symlink(s) => s.len() as u64,
            NodeData::Dir(d) => d.len() as u64 * 64,
        }
    }
}

struct TmpState {
    nodes: BTreeMap<u64, TmpNode>,
    next_ino: u64,
}

impl TmpState {
    fn new() -> Self {
        let mut s = TmpState { nodes: BTreeMap::new(), next_ino: 2 };
        s.nodes.insert(
            1,
            TmpNode {
                ino: 1,
                mode: S_IFDIR | 0o755,
                uid: 0,
                gid: 0,
                data: NodeData::Dir(BTreeMap::new()),
                atime: 0,
                mtime: 0,
                ctime: 0,
                nlink: 2,
            },
        );
        s
    }

    fn alloc_ino(&mut self) -> u64 {
        let ino = self.next_ino;
        self.next_ino += 1;
        ino
    }

    fn make_file(&mut self, mode: u32, uid: u32, gid: u32) -> u64 {
        let ino = self.alloc_ino();
        let t = crate::time::ticks() as i64;
        self.nodes.insert(
            ino,
            TmpNode {
                ino,
                mode: S_IFREG | (mode & 0o777),
                uid,
                gid,
                data: NodeData::File(TmpFileData::new()),
                atime: t,
                mtime: t,
                ctime: t,
                nlink: 1,
            },
        );
        ino
    }

    fn make_dir(&mut self, mode: u32, uid: u32, gid: u32) -> u64 {
        let ino = self.alloc_ino();
        let t = crate::time::ticks() as i64;
        self.nodes.insert(
            ino,
            TmpNode {
                ino,
                mode: S_IFDIR | (mode & 0o777),
                uid,
                gid,
                data: NodeData::Dir(BTreeMap::new()),
                atime: t,
                mtime: t,
                ctime: t,
                nlink: 2,
            },
        );
        ino
    }

    fn make_symlink(&mut self, target: &str, uid: u32, gid: u32) -> u64 {
        let ino = self.alloc_ino();
        let t = crate::time::ticks() as i64;
        self.nodes.insert(
            ino,
            TmpNode {
                ino,
                mode: S_IFLNK | 0o777,
                uid,
                gid,
                data: NodeData::Symlink(String::from(target)),
                atime: t,
                mtime: t,
                ctime: t,
                nlink: 1,
            },
        );
        ino
    }
}

pub struct TmpFs {
    state: Arc<Mutex<TmpState>>,
}

impl TmpFs {
    pub fn new() -> Superblock {
        let state = Arc::new(Mutex::new(TmpState::new()));
        Superblock {
            dev: 1,
            fs_type: String::from("tmpfs"),
            ops: Arc::new(TmpSbOps { state }),
        }
    }
}

struct TmpSbOps {
    state: Arc<Mutex<TmpState>>,
}

impl SuperblockOps for TmpSbOps {
    fn get_root(&self) -> Result<Inode, VfsError> {
        self.make_inode(1)
    }
}

impl TmpSbOps {
    fn make_inode(&self, ino: u64) -> Result<Inode, VfsError> {
        let state = self.state.lock();
        let node = state.nodes.get(&ino).ok_or(ENOENT)?;
        let (mode, uid, gid, size, at, mt, ct) =
            (node.mode, node.uid, node.gid, node.size(), node.atime, node.mtime, node.ctime);
        drop(state);

        Ok(Inode {
            ino,
            mode,
            uid,
            gid,
            size,
            atime: at,
            mtime: mt,
            ctime: ct,
            ops: Arc::new(TmpInoOps { state: self.state.clone(), ino }),
            sb: Arc::new(Superblock {
                dev: 1,
                fs_type: String::from("tmpfs"),
                ops: Arc::new(TmpSbOps { state: self.state.clone() }),
            }),
        })
    }
}

struct TmpInoOps {
    state: Arc<Mutex<TmpState>>,
    ino: u64,
}

impl InodeOps for TmpInoOps {
    fn read(&self, _: &Inode, buf: &mut [u8], offset: u64) -> Result<usize, VfsError> {
        let state = self.state.lock();
        let node = state.nodes.get(&self.ino).ok_or(ENOENT)?;
        match &node.data {
            NodeData::File(f) => Ok(f.read(offset as usize, buf)),
            NodeData::Symlink(t) => {
                let b = t.as_bytes();
                let n = buf.len().min(b.len());
                buf[..n].copy_from_slice(&b[..n]);
                Ok(n)
            }
            NodeData::Dir(_) => Err(EISDIR),
        }
    }

    fn write(&self, _: &Inode, data: &[u8], offset: u64) -> Result<usize, VfsError> {
        let s = offset as usize;
        let n = data.len();
        let end = s.checked_add(n).ok_or(EINVAL)?;

        crate::klog!("tmpfs_write: offset={}, data_len={}", s, n);

        let t = crate::time::ticks() as i64;
        let mut state = self.state.lock();
        match state.nodes.get_mut(&self.ino) {
            Some(node) => match &mut node.data {
                NodeData::File(file) => {
                    if end >= 64 * 1024 {
                        crate::klog!(
                            "tmpfs_write: page-backed write, {} bytes -> {} pages",
                            n,
                            TmpFileData::required_pages(end)
                        );
                    }
                    let written = file.write(s, data)?;
                    node.mtime = t;
                    node.ctime = t;
                    crate::klog!(
                        "tmpfs_write: complete, wrote {} bytes ({} pages resident)",
                        written,
                        file.page_count()
                    );
                    Ok(written)
                }
                NodeData::Dir(_) => Err(EISDIR),
                NodeData::Symlink(_) => Err(EINVAL),
            },
            None => Err(ENOENT),
        }
    }

    fn truncate(&self, _: &Inode, size: u64) -> Result<(), VfsError> {
        let mut state = self.state.lock();
        if let Some(node) = state.nodes.get_mut(&self.ino) {
            if let NodeData::File(file) = &mut node.data {
                file.truncate(size as usize)?;
                node.mtime = crate::time::ticks() as i64;
                node.ctime = node.mtime;
                return Ok(());
            }
        }
        Err(EINVAL)
    }

    fn readdir(&self, _: &Inode, _offset: u64) -> Result<Vec<DirEntry>, VfsError> {
        let state = self.state.lock();
        let node = state.nodes.get(&self.ino).ok_or(ENOENT)?;
        match &node.data {
            NodeData::Dir(children) => {
                let mut out = Vec::new();
                out.push(DirEntry { name: String::from("."), ino: self.ino, file_type: 4 });
                out.push(DirEntry { name: String::from(".."), ino: self.ino, file_type: 4 });
                for (name, &child_ino) in children {
                    let ftype = state
                        .nodes
                        .get(&child_ino)
                        .map(|n| match &n.data {
                            NodeData::Dir(_) => 4u8,
                            NodeData::Symlink(_) => 10u8,
                            NodeData::File(_) => 8u8,
                        })
                        .unwrap_or(8);
                    out.push(DirEntry { name: name.clone(), ino: child_ino, file_type: ftype });
                }
                Ok(out)
            }
            _ => Err(ENOTDIR),
        }
    }

    fn lookup(&self, _: &Inode, name: &str) -> Result<Inode, VfsError> {
        let child_ino = {
            let state = self.state.lock();
            let node = state.nodes.get(&self.ino).ok_or(ENOENT)?;
            match &node.data {
                NodeData::Dir(d) => *d.get(name).ok_or(ENOENT)?,
                _ => return Err(ENOTDIR),
            }
        };
        TmpSbOps { state: self.state.clone() }.make_inode(child_ino)
    }

    fn create(&self, _: &Inode, name: &str, mode: u32) -> Result<Inode, VfsError> {
        let (uid, gid) = crate::process::with_current(|p| (p.uid, p.gid)).unwrap_or((0, 0));
        let new_ino = {
            let mut state = self.state.lock();
            let ino = state.make_file(mode, uid, gid);
            match &mut state.nodes.get_mut(&self.ino).ok_or(ENOENT)?.data {
                NodeData::Dir(d) => {
                    d.insert(String::from(name), ino);
                }
                _ => return Err(ENOTDIR),
            }
            ino
        };
        TmpSbOps { state: self.state.clone() }.make_inode(new_ino)
    }

    fn mkdir(&self, _: &Inode, name: &str, mode: u32) -> Result<Inode, VfsError> {
        let (uid, gid) = crate::process::with_current(|p| (p.uid, p.gid)).unwrap_or((0, 0));
        let new_ino = {
            let mut state = self.state.lock();
            let ino = state.make_dir(mode, uid, gid);
            match &mut state.nodes.get_mut(&self.ino).ok_or(ENOENT)?.data {
                NodeData::Dir(d) => {
                    d.insert(String::from(name), ino);
                }
                _ => return Err(ENOTDIR),
            }
            ino
        };
        TmpSbOps { state: self.state.clone() }.make_inode(new_ino)
    }

    fn symlink(&self, _: &Inode, name: &str, target: &str) -> Result<Inode, VfsError> {
        let (uid, gid) = crate::process::with_current(|p| (p.uid, p.gid)).unwrap_or((0, 0));
        let new_ino = {
            let mut state = self.state.lock();
            let ino = state.make_symlink(target, uid, gid);
            match &mut state.nodes.get_mut(&self.ino).ok_or(ENOENT)?.data {
                NodeData::Dir(d) => {
                    d.insert(String::from(name), ino);
                }
                _ => return Err(ENOTDIR),
            }
            ino
        };
        TmpSbOps { state: self.state.clone() }.make_inode(new_ino)
    }

    fn unlink(&self, _: &Inode, name: &str) -> Result<(), VfsError> {
        let mut state = self.state.lock();
        let child_ino = match &state.nodes.get(&self.ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => *d.get(name).ok_or(ENOENT)?,
            _ => return Err(ENOTDIR),
        };
        if let NodeData::Dir(_) = &state.nodes.get(&child_ino).ok_or(ENOENT)?.data {
            return Err(EISDIR);
        }
        if let NodeData::Dir(d) = &mut state.nodes.get_mut(&self.ino).unwrap().data {
            d.remove(name);
        }
        if let Some(node) = state.nodes.get_mut(&child_ino) {
            if node.nlink > 0 {
                node.nlink -= 1;
            }
            if node.nlink == 0 {
                state.nodes.remove(&child_ino);
            }
        }
        Ok(())
    }

    fn rmdir(&self, _: &Inode, name: &str) -> Result<(), VfsError> {
        let mut state = self.state.lock();
        let child_ino = match &state.nodes.get(&self.ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => *d.get(name).ok_or(ENOENT)?,
            _ => return Err(ENOTDIR),
        };
        match &state.nodes.get(&child_ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => {
                if !d.is_empty() {
                    return Err(ENOTEMPTY);
                }
            }
            _ => return Err(ENOTDIR),
        }
        if let NodeData::Dir(d) = &mut state.nodes.get_mut(&self.ino).unwrap().data {
            d.remove(name);
        }
        state.nodes.remove(&child_ino);
        Ok(())
    }

    fn rename(&self, _: &Inode, old_name: &str, new_parent_ino: u64, new_name: &str) -> Result<(), VfsError> {
        let mut state = self.state.lock();
        let child_ino = match &state.nodes.get(&self.ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => *d.get(old_name).ok_or(ENOENT)?,
            _ => return Err(ENOTDIR),
        };
        if let NodeData::Dir(d) = &mut state.nodes.get_mut(&self.ino).unwrap().data {
            d.remove(old_name);
        }
        match &mut state.nodes.get_mut(&new_parent_ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => {
                d.insert(String::from(new_name), child_ino);
            }
            _ => return Err(ENOTDIR),
        }
        Ok(())
    }

    fn readlink(&self, _: &Inode) -> Result<String, VfsError> {
        let state = self.state.lock();
        match &state.nodes.get(&self.ino).ok_or(ENOENT)?.data {
            NodeData::Symlink(t) => Ok(t.clone()),
            _ => Err(EINVAL),
        }
    }
}
