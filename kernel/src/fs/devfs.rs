/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::vec;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::sync::Arc;
use crate::vfs::*;

pub struct DevFs;

impl DevFs {
    pub fn new() -> Superblock {
        Superblock { dev: 3, fs_type: String::from("devfs"), ops: Arc::new(DevFs) }
    }
}

impl SuperblockOps for DevFs {
    fn get_root(&self) -> Result<Inode, VfsError> {
        Ok(make_dir(1, Arc::new(DevRootOps)))
    }
}

fn make_dir(ino: u64, ops: Arc<dyn InodeOps>) -> Inode {
    Inode {
        ino, mode: S_IFDIR | 0o755, uid: 0, gid: 0, size: 0,
        atime: 0, mtime: 0, ctime: 0, ops,
        sb: Arc::new(Superblock { dev: 3, fs_type: String::from("devfs"), ops: Arc::new(DevFs) }),
    }
}

fn make_char(ino: u64, mode: u32, minor: u32) -> Inode {
    Inode {
        ino, mode, uid: 0, gid: 0, size: 0,
        atime: 0, mtime: 0, ctime: 0,
        ops: Arc::new(CharOps { minor }),
        sb: Arc::new(Superblock { dev: 3, fs_type: String::from("devfs"), ops: Arc::new(DevFs) }),
    }
}

fn make_drm_card(ino: u64) -> Inode {
    Inode {
        ino, mode: S_IFCHR | 0o666, uid: 0, gid: 0, size: 0,
        atime: 0, mtime: 0, ctime: 0,
        ops: Arc::new(DrmCardOps),
        sb: Arc::new(Superblock { dev: 3, fs_type: String::from("devfs"), ops: Arc::new(DevFs) }),
    }
}

// ── /dev root ────────────────────────────────────────────────────────────

struct DevRootOps;

impl InodeOps for DevRootOps {
    fn read(&self, _:&Inode, _:&mut [u8], _:u64) -> Result<usize, VfsError> { Err(EISDIR) }
    fn write(&self, _:&Inode, _:&[u8], _:u64) -> Result<usize, VfsError> { Err(EISDIR) }

    fn readdir(&self, _:&Inode, _:u64) -> Result<Vec<DirEntry>, VfsError> {
        let mut v = vec![
            DirEntry { name: String::from("."),       ino: 1, file_type: 4 },
            DirEntry { name: String::from(".."),      ino: 1, file_type: 4 },
            DirEntry { name: String::from("null"),    ino: 2, file_type: 2 },
            DirEntry { name: String::from("zero"),    ino: 3, file_type: 2 },
            DirEntry { name: String::from("tty"),     ino: 4, file_type: 2 },
            DirEntry { name: String::from("console"), ino: 5, file_type: 2 },
            DirEntry { name: String::from("serial"),  ino: 6, file_type: 2 },
            DirEntry { name: String::from("dri"),     ino: 7, file_type: 4 },
        ];
        Ok(v)
    }

    fn lookup(&self, _:&Inode, name: &str) -> Result<Inode, VfsError> {
        match name {
            "null"    => Ok(make_char(2, S_IFCHR|0o666, 0)),
            "zero"    => Ok(make_char(3, S_IFCHR|0o666, 1)),
            "tty"     => Ok(make_char(4, S_IFCHR|0o666, 2)),
            "console" => Ok(make_char(5, S_IFCHR|0o600, 3)),
            "serial"  => Ok(make_char(6, S_IFCHR|0o600, 4)),
            "dri"     => Ok(make_dir(7, Arc::new(DriDirOps))),
            _         => Err(ENOENT),
        }
    }
}

// ── /dev/dri/ ─────────────────────────────────────────────────────────────

struct DriDirOps;

impl InodeOps for DriDirOps {
    fn read(&self, _:&Inode, _:&mut [u8], _:u64) -> Result<usize, VfsError> { Err(EISDIR) }
    fn write(&self, _:&Inode, _:&[u8], _:u64) -> Result<usize, VfsError> { Err(EISDIR) }

    fn readdir(&self, _:&Inode, _:u64) -> Result<Vec<DirEntry>, VfsError> {
        Ok(vec![
            DirEntry { name: String::from("."),      ino: 7,  file_type: 4 },
            DirEntry { name: String::from(".."),     ino: 1,  file_type: 4 },
            DirEntry { name: String::from("card0"),  ino: 8,  file_type: 2 },
            DirEntry { name: String::from("renderD128"), ino: 9, file_type: 2 },
        ])
    }

    fn lookup(&self, _:&Inode, name: &str) -> Result<Inode, VfsError> {
        match name {
            "card0"      => Ok(make_drm_card(8)),
            "renderD128" => Ok(make_drm_card(9)),
            _            => Err(ENOENT),
        }
    }
}

// ── /dev/dri/card0 operations ─────────────────────────────────────────────

struct DrmCardOps;

impl InodeOps for DrmCardOps {
    fn read(&self, _:&Inode, _:&mut [u8], _:u64) -> Result<usize, VfsError> { Ok(0) }
    fn write(&self, _:&Inode, _:&[u8], _:u64) -> Result<usize, VfsError> { Ok(0) }
    fn readdir(&self, _:&Inode, _:u64) -> Result<Vec<DirEntry>, VfsError> { Err(ENOTDIR) }
    fn lookup(&self, _:&Inode, _:&str) -> Result<Inode, VfsError> { Err(ENOTDIR) }
}

// ── Regular character device ops (null/zero/tty) ──────────────────────────

struct CharOps { minor: u32 }

impl InodeOps for CharOps {
    fn read(&self, _:&Inode, buf:&mut [u8], off:u64) -> Result<usize, VfsError> {
        crate::device::read_device(self.minor, buf.as_mut_ptr(), buf.len(), off)
    }
    fn write(&self, _:&Inode, buf:&[u8], off:u64) -> Result<usize, VfsError> {
        crate::device::write_device(self.minor, buf.as_ptr(), buf.len(), off)
    }
    fn readdir(&self, _:&Inode, _:u64) -> Result<Vec<DirEntry>, VfsError> { Err(ENOTDIR) }
    fn lookup(&self, _:&Inode, _:&str) -> Result<Inode, VfsError> { Err(ENOTDIR) }
    fn device_minor(&self) -> Option<u32> { Some(self.minor) }
}

// ── FdKind for DRM file descriptors ──────────────────────────────────────
// When userland opens /dev/dri/card0 we create a FileDescriptor with kind = Drm.
// The ioctl and mmap paths dispatch on this kind.

pub fn open_drm_card() -> FileDescriptor {
    FileDescriptor {
        inode: make_drm_card(8),
        offset: 0,
        flags: 2,                      // O_RDWR
        kind: FdKind::Drm,
        path: alloc::string::String::new(),
    }
}
