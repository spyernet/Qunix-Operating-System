/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::arch::x86_64::paging::{PAGE_SIZE, phys_to_virt};
use crate::memory::phys::{alloc_frame, free_frame};
use crate::memory::vmm::Prot;

pub struct ShmRegion {
    pub name:   String,
    pub frames: Vec<u64>,
    pub size:   u64,
    pub refs:   u32,
    pub mode:   u32,
}

static SHM_MAP: Mutex<BTreeMap<String, ShmRegion>> = Mutex::new(BTreeMap::new());

pub fn shm_open(name: &str, flags: i32, mode: u32) -> i64 {
    let create = flags & 0o100 != 0;  // O_CREAT
    let excl   = flags & 0o200 != 0;  // O_EXCL

    let mut map = SHM_MAP.lock();
    if map.contains_key(name) {
        if create && excl { return -17; } // EEXIST
        map.get_mut(name).unwrap().refs += 1;
    } else {
        if !create { return -2; } // ENOENT
        map.insert(String::from(name), ShmRegion {
            name:   String::from(name),
            frames: Vec::new(),
            size:   0,
            refs:   1,
            mode,
        });
    }

    // Allocate fd pointing at this shm object
    let fd = alloc_shm_fd(name);
    fd
}

pub fn shm_unlink(name: &str) -> i64 {
    let mut map = SHM_MAP.lock();
    if map.remove(name).is_some() { 0 } else { -2 }
}

pub fn ftruncate_shm(name: &str, size: u64) -> i64 {
    let mut map = SHM_MAP.lock();
    let region = match map.get_mut(name) {
        Some(r) => r,
        None    => return -9,
    };

    let needed = ((size + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
    let current = region.frames.len();

    if needed > current {
        for _ in current..needed {
            match alloc_frame() {
                Some(f) => {
                    unsafe { core::ptr::write_bytes(phys_to_virt(f) as *mut u8, 0, PAGE_SIZE as usize); }
                    region.frames.push(f);
                }
                None => return -12,
            }
        }
    } else {
        for f in region.frames.drain(needed..) {
            free_frame(f);
        }
    }
    region.size = size;
    0
}

pub fn mmap_shm(name: &str, offset: u64, len: u64, prot: Prot) -> Option<u64> {
    let map = SHM_MAP.lock();
    let region = map.get(name)?;

    use crate::arch::x86_64::paging::PageFlags;
    let mut mapper = crate::arch::x86_64::paging::PageMapper::new(
        crate::arch::x86_64::paging::get_cr3()
    );

    let addr = crate::process::with_current_mut(|p| p.address_space.mmap_base);
    let pages = (len + PAGE_SIZE - 1) / PAGE_SIZE;

    let start_page = (offset / PAGE_SIZE) as usize;
    let mut flags = PageFlags::PRESENT | PageFlags::USER;
    if prot.contains(Prot::WRITE) { flags |= PageFlags::WRITABLE; }
    if !prot.contains(Prot::EXEC) { flags |= PageFlags::NO_EXECUTE; }

    for i in 0..pages as usize {
        let fidx = start_page + i;
        if fidx < region.frames.len() {
            if let Some(base) = addr { unsafe { mapper.map_page(base + i as u64 * PAGE_SIZE, region.frames[fidx], flags); } }
        }
    }

    crate::process::with_current_mut(|p| {
        p.address_space.mmap_base += pages * PAGE_SIZE + PAGE_SIZE;
        p.address_space.regions.push(crate::memory::vmm::VmaRegion {
            start: addr.unwrap_or(0),
            end:   addr.unwrap_or(0) + pages * PAGE_SIZE,
            prot,
            kind:  crate::memory::vmm::RegionKind::Shared,
            flags: crate::memory::vmm::MAP_SHARED,
            name:  alloc::string::String::new(),
            cow:   false,
        });
    });

    addr
}

static SHM_FD_MAP: Mutex<BTreeMap<u32, String>> = Mutex::new(BTreeMap::new());
static NEXT_SHM_FD: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(500);

fn alloc_shm_fd(name: &str) -> i64 {
    use crate::vfs::{FileDescriptor, FdKind, Inode, InodeOps, DirEntry, VfsError};
    use alloc::sync::Arc;

    struct ShmIno { name: String }
    impl InodeOps for ShmIno {
        fn read(&self, _: &Inode, _: &mut [u8], _: u64) -> Result<usize, VfsError> { Ok(0) }
        fn write(&self, _: &Inode, _: &[u8], _: u64) -> Result<usize, VfsError> { Ok(0) }
        fn readdir(&self, _: &Inode, _: u64) -> Result<Vec<DirEntry>, VfsError> { Err(crate::vfs::ENOTDIR) }
        fn lookup(&self, _: &Inode, _: &str) -> Result<Inode, VfsError> { Err(crate::vfs::ENOENT) }
    }

    struct NullSb2;
    impl crate::vfs::SuperblockOps for NullSb2 {
        fn get_root(&self) -> Result<Inode, VfsError> { Err(crate::vfs::ENOENT) }
    }

    let size = SHM_MAP.lock().get(name).map(|r| r.size).unwrap_or(0);
    let inode = Inode {
        ino: 0xA000_0000,
        mode: crate::vfs::S_IFREG | 0o600,
        uid: 0, gid: 0, size,
        atime: 0, mtime: 0, ctime: 0,
        ops: Arc::new(ShmIno { name: String::from(name) }),
        sb: Arc::new(crate::vfs::Superblock {
            dev: 10, fs_type: String::from("shmfs"), ops: Arc::new(NullSb2),
        }),
    };

    let fd = FileDescriptor { inode, offset: 0, flags: 0, kind: FdKind::Regular, path: alloc::string::String::new(),};
    crate::process::with_current_mut(|p| p.alloc_fd(fd)).map(|n| n as i64).unwrap_or(-9)
}
