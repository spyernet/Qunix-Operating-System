//! tmpfs — memory-backed filesystem, full read/write/create/mkdir/unlink.
//!
//! Each file's data lives in a Vec<u8> inside a locked TmpFsNode.
//! Node IDs are globally unique within the instance.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;
use crate::vfs::*;

// ── Node types ────────────────────────────────────────────────────────────

#[derive(Clone)]
enum NodeData {
    File(Vec<u8>),
    Dir(BTreeMap<String, u64>),   // name -> inode number
    Symlink(String),
}

struct TmpNode {
    ino:   u64,
    mode:  u32,
    uid:   u32,
    gid:   u32,
    data:  NodeData,
    atime: i64,
    mtime: i64,
    ctime: i64,
    nlink: u32,
}

impl TmpNode {
    fn size(&self) -> u64 {
        match &self.data {
            NodeData::File(v)    => v.len() as u64,
            NodeData::Symlink(s) => s.len() as u64,
            NodeData::Dir(d)     => d.len() as u64 * 64,
        }
    }
}

// ── Filesystem state ──────────────────────────────────────────────────────

struct TmpState {
    nodes:    BTreeMap<u64, TmpNode>,
    next_ino: u64,
}

impl TmpState {
    fn new() -> Self {
        let mut s = TmpState { nodes: BTreeMap::new(), next_ino: 2 };
        s.nodes.insert(1, TmpNode {
            ino: 1, mode: S_IFDIR | 0o755, uid: 0, gid: 0,
            data: NodeData::Dir(BTreeMap::new()),
            atime: 0, mtime: 0, ctime: 0, nlink: 2,
        });
        s
    }

    fn alloc_ino(&mut self) -> u64 {
        let ino = self.next_ino;
        self.next_ino += 1;
        ino
    }

    fn make_file(&mut self, mode: u32, uid: u32, gid: u32) -> u64 {
        let ino = self.alloc_ino();
        let t   = crate::time::ticks() as i64;
        self.nodes.insert(ino, TmpNode {
            ino, mode: S_IFREG | (mode & 0o777), uid, gid,
            data: NodeData::File(Vec::new()),
            atime: t, mtime: t, ctime: t, nlink: 1,
        });
        ino
    }

    fn make_dir(&mut self, mode: u32, uid: u32, gid: u32) -> u64 {
        let ino = self.alloc_ino();
        let t   = crate::time::ticks() as i64;
        self.nodes.insert(ino, TmpNode {
            ino, mode: S_IFDIR | (mode & 0o777), uid, gid,
            data: NodeData::Dir(BTreeMap::new()),
            atime: t, mtime: t, ctime: t, nlink: 2,
        });
        ino
    }

    fn make_symlink(&mut self, target: &str, uid: u32, gid: u32) -> u64 {
        let ino = self.alloc_ino();
        let t   = crate::time::ticks() as i64;
        self.nodes.insert(ino, TmpNode {
            ino, mode: S_IFLNK | 0o777, uid, gid,
            data: NodeData::Symlink(String::from(target)),
            atime: t, mtime: t, ctime: t, nlink: 1,
        });
        ino
    }
}

// ── Superblock + inode builders ───────────────────────────────────────────

pub struct TmpFs {
    state: Arc<Mutex<TmpState>>,
    dev:   u64,
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

struct TmpSbOps { state: Arc<Mutex<TmpState>> }

impl SuperblockOps for TmpSbOps {
    fn get_root(&self) -> Result<Inode, VfsError> {
        self.make_inode(1)
    }
}

impl TmpSbOps {
    fn make_inode(&self, ino: u64) -> Result<Inode, VfsError> {
        let state = self.state.lock();
        let node  = state.nodes.get(&ino).ok_or(ENOENT)?;
        let (mode, uid, gid, size, at, mt, ct) =
            (node.mode, node.uid, node.gid, node.size(), node.atime, node.mtime, node.ctime);
        drop(state);
        Ok(Inode {
            ino, mode, uid, gid, size,
            atime: at, mtime: mt, ctime: ct,
            ops: Arc::new(TmpInoOps { state: self.state.clone(), ino }),
            sb:  Arc::new(Superblock {
                dev: 1, fs_type: String::from("tmpfs"),
                ops: Arc::new(TmpSbOps { state: self.state.clone() }),
            }),
        })
    }
}

// ── Inode ops ─────────────────────────────────────────────────────────────

struct TmpInoOps { state: Arc<Mutex<TmpState>>, ino: u64 }

impl InodeOps for TmpInoOps {
    fn read(&self, inode: &Inode, buf: &mut [u8], offset: u64) -> Result<usize, VfsError> {
        let state = self.state.lock();
        let node  = state.nodes.get(&self.ino).ok_or(ENOENT)?;
        match &node.data {
            NodeData::File(v) => {
                let s = offset as usize;
                if s >= v.len() { return Ok(0); }
                let n = buf.len().min(v.len() - s);
                buf[..n].copy_from_slice(&v[s..s + n]);
                Ok(n)
            }
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
        let mut state = self.state.lock();
        let node = state.nodes.get_mut(&self.ino).ok_or(ENOENT)?;
        let t = crate::time::ticks() as i64;
        node.mtime = t;
        node.ctime = t;
        match &mut node.data {
            NodeData::File(v) => {
                let s   = offset as usize;
                let end = s + data.len();
                if end > v.len() { v.resize(end, 0); }
                v[s..end].copy_from_slice(data);
                Ok(data.len())
            }
            NodeData::Dir(_) => Err(EISDIR),
            _                => Err(EINVAL),
        }
    }

    fn truncate(&self, _: &Inode, size: u64) -> Result<(), VfsError> {
        let mut state = self.state.lock();
        if let Some(node) = state.nodes.get_mut(&self.ino) {
            if let NodeData::File(v) = &mut node.data {
                v.resize(size as usize, 0);
                node.mtime = crate::time::ticks() as i64;
                return Ok(());
            }
        }
        Err(EINVAL)
    }

    fn readdir(&self, _: &Inode, _offset: u64) -> Result<Vec<DirEntry>, VfsError> {
        let state = self.state.lock();
        let node  = state.nodes.get(&self.ino).ok_or(ENOENT)?;
        match &node.data {
            NodeData::Dir(children) => {
                let mut out = Vec::new();
                out.push(DirEntry { name: String::from("."),  ino: self.ino, file_type: 4 });
                out.push(DirEntry { name: String::from(".."), ino: self.ino, file_type: 4 });
                for (name, &child_ino) in children {
                    let ftype = state.nodes.get(&child_ino).map(|n| {
                        match &n.data {
                            NodeData::Dir(_)     => 4u8,
                            NodeData::Symlink(_) => 10u8,
                            NodeData::File(_)    => 8u8,
                        }
                    }).unwrap_or(8);
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
            let node  = state.nodes.get(&self.ino).ok_or(ENOENT)?;
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
                NodeData::Dir(d) => { d.insert(String::from(name), ino); }
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
                NodeData::Dir(d) => { d.insert(String::from(name), ino); }
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
                NodeData::Dir(d) => { d.insert(String::from(name), ino); }
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
        // Decrement link count; remove if zero
        if let Some(node) = state.nodes.get_mut(&child_ino) {
            if node.nlink > 0 { node.nlink -= 1; }
            if node.nlink == 0 { state.nodes.remove(&child_ino); }
        }
        Ok(())
    }

    fn rmdir(&self, _: &Inode, name: &str) -> Result<(), VfsError> {
        let mut state = self.state.lock();
        let child_ino = match &state.nodes.get(&self.ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => *d.get(name).ok_or(ENOENT)?,
            _ => return Err(ENOTDIR),
        };
        // Directory must be empty
        match &state.nodes.get(&child_ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => if !d.is_empty() { return Err(ENOTEMPTY); }
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
        // Get child ino
        let child_ino = match &state.nodes.get(&self.ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => *d.get(old_name).ok_or(ENOENT)?,
            _ => return Err(ENOTDIR),
        };
        // Remove from old dir
        if let NodeData::Dir(d) = &mut state.nodes.get_mut(&self.ino).unwrap().data {
            d.remove(old_name);
        }
        // Insert into new dir
        match &mut state.nodes.get_mut(&new_parent_ino).ok_or(ENOENT)?.data {
            NodeData::Dir(d) => { d.insert(String::from(new_name), child_ino); }
            _ => return Err(ENOTDIR),
        }
        Ok(())
    }

    fn readlink(&self, _: &Inode) -> Result<String, VfsError> {
        let state = self.state.lock();
        match &state.nodes.get(&self.ino).ok_or(ENOENT)?.data {
            NodeData::Symlink(t) => Ok(t.clone()),
            _                   => Err(EINVAL),
        }
    }
}
