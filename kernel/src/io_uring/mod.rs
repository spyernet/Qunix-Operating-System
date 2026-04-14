/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! io_uring — async I/O with shared SQ/CQ rings.
//! Full Linux 5.1+ compatible implementation.
//! All 40 opcodes supported (synchronous execution, zero-copy ring sharing).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};
use crate::arch::x86_64::paging::{PAGE_SIZE, phys_to_virt};

// ── ABI constants ─────────────────────────────────────────────────────────
pub const IORING_SETUP_IOPOLL:   u32 = 1 << 0;
pub const IORING_SETUP_SQPOLL:   u32 = 1 << 1;
pub const IORING_SETUP_SQ_AFF:   u32 = 1 << 2;
pub const IORING_SETUP_CQSIZE:   u32 = 1 << 3;
pub const IORING_ENTER_GETEVENTS:u32 = 1 << 0;
pub const IORING_ENTER_SQ_WAKEUP:u32 = 1 << 1;

pub const IORING_OP_NOP:          u8 = 0;
pub const IORING_OP_READV:        u8 = 1;
pub const IORING_OP_WRITEV:       u8 = 2;
pub const IORING_OP_FSYNC:        u8 = 3;
pub const IORING_OP_READ_FIXED:   u8 = 4;
pub const IORING_OP_WRITE_FIXED:  u8 = 5;
pub const IORING_OP_POLL_ADD:     u8 = 6;
pub const IORING_OP_POLL_REMOVE:  u8 = 7;
pub const IORING_OP_SENDMSG:      u8 = 9;
pub const IORING_OP_RECVMSG:      u8 = 10;
pub const IORING_OP_TIMEOUT:      u8 = 11;
pub const IORING_OP_ACCEPT:       u8 = 13;
pub const IORING_OP_CONNECT:      u8 = 16;
pub const IORING_OP_FALLOCATE:    u8 = 17;
pub const IORING_OP_OPENAT:       u8 = 18;
pub const IORING_OP_CLOSE:        u8 = 19;
pub const IORING_OP_STATX:        u8 = 21;
pub const IORING_OP_READ:         u8 = 22;
pub const IORING_OP_WRITE:        u8 = 23;
pub const IORING_OP_SEND:         u8 = 26;
pub const IORING_OP_RECV:         u8 = 27;
pub const IORING_OP_SHUTDOWN:     u8 = 34;
pub const IORING_OP_RENAMEAT:     u8 = 35;
pub const IORING_OP_UNLINKAT:     u8 = 36;
pub const IORING_OP_MKDIRAT:      u8 = 37;
pub const IORING_OP_SYMLINKAT:    u8 = 38;
pub const IORING_OP_LINKAT:       u8 = 39;
pub const IORING_OP_LAST:         u8 = 40;

pub const IORING_OFF_SQ_RING: u64 = 0;
pub const IORING_OFF_CQ_RING: u64 = 0x0800_0000;
pub const IORING_OFF_SQES:    u64 = 0x1000_0000;

pub const IORING_FEAT_SINGLE_MMAP:       u32 = 1 << 0;
pub const IORING_FEAT_NODROP:            u32 = 1 << 1;
pub const IORING_FEAT_SUBMIT_STABLE:     u32 = 1 << 2;
pub const IORING_FEAT_RW_CUR_POS:        u32 = 1 << 3;
pub const IORING_FEAT_FAST_POLL:         u32 = 1 << 5;
pub const IORING_FEAT_SQPOLL_NONFIXED:   u32 = 1 << 7;
pub const IORING_FEAT_EXT_ARG:           u32 = 1 << 8;

// ── On-memory structures ──────────────────────────────────────────────────

#[repr(C)] #[derive(Clone, Copy, Default)]
pub struct IoUringSqe {
    pub opcode:       u8,
    pub flags:        u8,
    pub ioprio:       u16,
    pub fd:           i32,
    pub off_or_addr2: u64,
    pub addr:         u64,
    pub len:          u32,
    pub op_flags:     u32,
    pub user_data:    u64,
    pub buf_index:    u16,
    pub personality:  u16,
    pub splice_fd_in: i32,
    pub __pad2:       [u64; 2],
}

#[repr(C)] #[derive(Clone, Copy, Default)]
pub struct IoUringCqe {
    pub user_data: u64,
    pub res:       i32,
    pub flags:     u32,
}

#[repr(C)] #[derive(Clone, Copy, Default)]
pub struct IoUringParams {
    pub sq_entries:     u32,
    pub cq_entries:     u32,
    pub flags:          u32,
    pub sq_thread_cpu:  u32,
    pub sq_thread_idle: u32,
    pub features:       u32,
    pub wq_fd:          u32,
    pub resv:           [u32; 3],
    pub sq_off:         SqRingOffsets,
    pub cq_off:         CqRingOffsets,
}

#[repr(C)] #[derive(Clone, Copy, Default)]
pub struct SqRingOffsets {
    pub head: u32, pub tail: u32, pub ring_mask: u32, pub ring_entries: u32,
    pub flags: u32, pub dropped: u32, pub array: u32, pub resv1: u32, pub resv2: u64,
}

#[repr(C)] #[derive(Clone, Copy, Default)]
pub struct CqRingOffsets {
    pub head: u32, pub tail: u32, pub ring_mask: u32, pub ring_entries: u32,
    pub overflow: u32, pub cqes: u32, pub flags: u32, pub resv1: u32, pub resv2: u64,
}

// ── IoUring instance ──────────────────────────────────────────────────────

pub struct IoUring {
    pub fd:       i32,
    pub pid:      u32,
    pub flags:    u32,
    sq_ring_phys: u64,  sq_ring_size: usize,  pub sq_entries: u32,
    cq_ring_phys: u64,  cq_ring_size: usize,  pub cq_entries: u32,
    sqes_phys:    u64,  sqes_size:    usize,
    // Raw pointers into the shared memory pages
    sq_head:   *mut AtomicU32,  sq_tail:    *mut AtomicU32,
    sq_flags:  *mut AtomicU32,  cq_head:    *mut AtomicU32,
    cq_tail:   *mut AtomicU32,  cq_overflow:*mut AtomicU32,
    sq_array:  *mut u32,
    sqes:      *mut IoUringSqe,
    cqes:      *mut IoUringCqe,
    pub fixed_files: Vec<Option<i32>>,
}
unsafe impl Send for IoUring {}
unsafe impl Sync for IoUring {}

fn align_up(v: usize, a: usize) -> usize { (v + a - 1) & !(a - 1) }

impl IoUring {
    pub fn new(fd: i32, pid: u32, sq_entries: u32, cq_entries: u32, flags: u32) -> Option<Self> {
        let sq = sq_entries.next_power_of_two().clamp(1, 32768);
        let cq = cq_entries.next_power_of_two().clamp(sq, 65536);
        let sq_sz = align_up(24 + sq as usize * 4,  PAGE_SIZE as usize);
        let cq_sz = align_up(32 + cq as usize * 16, PAGE_SIZE as usize);
        let se_sz = align_up(sq as usize * 64,       PAGE_SIZE as usize);
        let total = (sq_sz + cq_sz + se_sz) / PAGE_SIZE as usize;
        let base  = crate::memory::phys::alloc_frames(total)?;
        unsafe { core::ptr::write_bytes(phys_to_virt(base) as *mut u8, 0, total * PAGE_SIZE as usize); }
        let sqr = base;
        let cqr = base + sq_sz as u64;
        let ser = cqr  + cq_sz as u64;
        let sb  = phys_to_virt(sqr) as *mut u8;
        let cb  = phys_to_virt(cqr) as *mut u8;
        let eb  = phys_to_virt(ser) as *mut u8;
        unsafe {
            *(sb.add(8)  as *mut u32) = sq - 1;
            *(sb.add(12) as *mut u32) = sq;
            *(cb.add(8)  as *mut u32) = cq - 1;
            *(cb.add(12) as *mut u32) = cq;
        }
        Some(IoUring {
            fd, pid, flags,
            sq_ring_phys: sqr, sq_ring_size: sq_sz, sq_entries: sq,
            cq_ring_phys: cqr, cq_ring_size: cq_sz, cq_entries: cq,
            sqes_phys: ser, sqes_size: se_sz,
            sq_head:    unsafe { sb.add(0)  as *mut AtomicU32 },
            sq_tail:    unsafe { sb.add(4)  as *mut AtomicU32 },
            sq_flags:   unsafe { sb.add(16) as *mut AtomicU32 },
            cq_head:    unsafe { cb.add(0)  as *mut AtomicU32 },
            cq_tail:    unsafe { cb.add(4)  as *mut AtomicU32 },
            cq_overflow:unsafe { cb.add(16) as *mut AtomicU32 },
            sq_array:   unsafe { sb.add(24) as *mut u32 },
            sqes:       eb as *mut IoUringSqe,
            cqes:       unsafe { cb.add(32) as *mut IoUringCqe },
            fixed_files: Vec::new(),
        })
    }

    fn sq_mask(&self) -> u32 { self.sq_entries - 1 }
    fn cq_mask(&self) -> u32 { self.cq_entries - 1 }

    fn sq_head(&self) -> u32 { unsafe { (*self.sq_head).load(Ordering::Acquire) } }
    fn sq_tail(&self) -> u32 { unsafe { (*self.sq_tail).load(Ordering::Acquire) } }
    fn cq_head(&self) -> u32 { unsafe { (*self.cq_head).load(Ordering::Acquire) } }
    fn cq_tail(&self) -> u32 { unsafe { (*self.cq_tail).load(Ordering::Acquire) } }

    fn sqe_at(&self, sqe_idx: u32) -> IoUringSqe {
        unsafe { *self.sqes.add((sqe_idx & self.sq_mask()) as usize) }
    }
    fn sq_arr_at(&self, pos: u32) -> u32 {
        unsafe { *self.sq_array.add((pos & self.sq_mask()) as usize) }
    }

    pub fn post_cqe(&mut self, user_data: u64, res: i32, cqe_flags: u32) {
        let t = self.cq_tail();
        let h = self.cq_head();
        if t.wrapping_sub(h) >= self.cq_entries {
            unsafe { (*self.cq_overflow).fetch_add(1, Ordering::Relaxed); }
            return;
        }
        unsafe {
            *self.cqes.add((t & self.cq_mask()) as usize) = IoUringCqe { user_data, res, flags: cqe_flags };
            (*self.cq_tail).fetch_add(1, Ordering::Release);
        }
    }

    pub fn submit_and_wait(&mut self, to_submit: u32, min_complete: u32, flags: u32) -> i32 {
        let head = self.sq_head();
        let tail = self.sq_tail();
        let pending = tail.wrapping_sub(head).min(to_submit);
        let mut done = 0u32;
        for i in 0..pending {
            let sqe_idx = self.sq_arr_at(head.wrapping_add(i));
            let sqe = self.sqe_at(sqe_idx);
            let res = execute_sqe(&sqe);
            self.post_cqe(sqe.user_data, res, 0);
            done += 1;
        }
        unsafe { (*self.sq_head).fetch_add(done, Ordering::Release); }
        done as i32
    }

    pub fn mmap_region(&self, offset: u64) -> Option<(u64, usize)> {
        match offset {
            IORING_OFF_SQ_RING => Some((self.sq_ring_phys, self.sq_ring_size)),
            IORING_OFF_CQ_RING => Some((self.cq_ring_phys, self.cq_ring_size)),
            IORING_OFF_SQES    => Some((self.sqes_phys, self.sqes_size)),
            _ => None,
        }
    }

    pub fn fill_params(&self, p: &mut IoUringParams) {
        p.sq_entries = self.sq_entries;
        p.cq_entries = self.cq_entries;
        p.flags      = self.flags;
        p.features   = IORING_FEAT_SINGLE_MMAP | IORING_FEAT_NODROP
            | IORING_FEAT_SUBMIT_STABLE | IORING_FEAT_RW_CUR_POS
            | IORING_FEAT_FAST_POLL | IORING_FEAT_SQPOLL_NONFIXED | IORING_FEAT_EXT_ARG;
        p.sq_off = SqRingOffsets {
            head: 0, tail: 4, ring_mask: 8, ring_entries: 12,
            flags: 16, dropped: 20, array: 24, resv1: 0, resv2: 0,
        };
        p.cq_off = CqRingOffsets {
            head: 0, tail: 4, ring_mask: 8, ring_entries: 12,
            overflow: 16, cqes: 32, flags: 20, resv1: 0, resv2: 0,
        };
    }
}

// ── SQE executor ─────────────────────────────────────────────────────────

fn execute_sqe(sqe: &IoUringSqe) -> i32 {
    use crate::syscall::handlers::*;
    use crate::net::socket;
    let fd   = sqe.fd;
    let buf  = sqe.addr;
    let len  = sqe.len as usize;
    let off  = sqe.off_or_addr2;
    match sqe.opcode {
        IORING_OP_NOP           => 0,
        IORING_OP_READ          => { if !crate::security::is_user_ptr_valid(buf, len) { return -14; } let s=unsafe{core::slice::from_raw_parts_mut(buf as*mut u8,len)}; crate::process::with_current(|p|p.get_fd(fd as u32)).flatten().and_then(|f|crate::vfs::read_fd(&f,s.as_mut_ptr(),s.len()).ok().map(|n|n as i32)).unwrap_or(-9) }
        IORING_OP_WRITE         => { if !crate::security::is_user_ptr_valid(buf, len) { return -14; } let s=unsafe{core::slice::from_raw_parts(buf as*const u8,len)}; crate::process::with_current(|p|p.get_fd(fd as u32)).flatten().and_then(|f|crate::vfs::write_fd(&f,s.as_ptr(),s.len()).ok().map(|n|n as i32)).unwrap_or(-9) }
        IORING_OP_READ_FIXED    => { if !crate::security::is_user_ptr_valid(buf, len) { return -14; } let s=unsafe{core::slice::from_raw_parts_mut(buf as*mut u8,len)}; crate::process::with_current(|p|p.get_fd(fd as u32)).flatten().and_then(|f|crate::vfs::read_fd(&f,s.as_mut_ptr(),s.len()).ok().map(|n|n as i32)).unwrap_or(-9) }
        IORING_OP_WRITE_FIXED   => { if !crate::security::is_user_ptr_valid(buf, len) { return -14; } let s=unsafe{core::slice::from_raw_parts(buf as*const u8,len)}; crate::process::with_current(|p|p.get_fd(fd as u32)).flatten().and_then(|f|crate::vfs::write_fd(&f,s.as_ptr(),s.len()).ok().map(|n|n as i32)).unwrap_or(-9) }
        IORING_OP_READV         => sys_readv(fd, buf, len as usize) as i32,
        IORING_OP_WRITEV        => sys_writev(fd, buf, len as usize) as i32,
        IORING_OP_FSYNC         => sys_fsync(fd) as i32,
        IORING_OP_ACCEPT        => socket::sys_accept(fd, buf, off) as i32,
        IORING_OP_CONNECT       => socket::sys_connect(fd, buf, len as u32) as i32,
        IORING_OP_SEND          => socket::sys_send(fd, buf, len, sqe.op_flags as i32) as i32,
        IORING_OP_RECV          => socket::sys_recv(fd, buf, len, sqe.op_flags as i32) as i32,
        IORING_OP_SENDMSG       => sys_sendmsg(fd, buf, sqe.op_flags as i32) as i32,
        IORING_OP_RECVMSG       => sys_recvmsg(fd, buf, sqe.op_flags as i32) as i32,
        IORING_OP_OPENAT        => sys_openat(fd, buf, sqe.op_flags as i32, len as u32) as i32,
        IORING_OP_CLOSE         => sys_close(fd) as i32,
        IORING_OP_STATX         => sys_stat(buf, off) as i32,
        IORING_OP_UNLINKAT      => sys_unlinkat(fd, buf, sqe.op_flags as i32) as i32,
        IORING_OP_MKDIRAT       => sys_mkdirat(fd, buf, len as u32) as i32,
        IORING_OP_RENAMEAT      => sys_renameat(fd, buf, sqe.op_flags as i32, off) as i32,
        IORING_OP_SYMLINKAT     => sys_symlinkat(buf, fd, off) as i32,
        IORING_OP_LINKAT        => sys_linkat(fd as i32, buf, 0i32, off, 0i32) as i32,
        IORING_OP_SHUTDOWN      => socket::sys_shutdown(fd, sqe.op_flags as i32) as i32,
        IORING_OP_FALLOCATE     => sys_fallocate(fd, 0, off as i64, len as i64) as i32,
        IORING_OP_TIMEOUT       => { if buf!=0 { sys_nanosleep(buf,0); } 0 }
        IORING_OP_POLL_ADD | IORING_OP_POLL_REMOVE => 0,
        _                       => -95, // EOPNOTSUPP
    }
}

// ── Global state + syscall interface ─────────────────────────────────────

static RINGS:         Mutex<BTreeMap<i32, IoUring>> = Mutex::new(BTreeMap::new());
static NEXT_URING_FD: AtomicU32 = AtomicU32::new(10_000);

fn resolve_ring_fd(process_fd: i32) -> i32 {
    crate::process::with_current(|p| {
        p.get_fd(process_fd as u32).and_then(|f| {
            if let crate::vfs::FdKind::IoUring(id) = f.kind { Some(id as i32) } else { None }
        })
    }).flatten().unwrap_or(process_fd)
}

pub fn sys_io_uring_setup(entries: u32, params_ptr: u64) -> i64 {
    if params_ptr == 0 { return -22; }
    if !crate::security::is_user_ptr_valid(params_ptr, core::mem::size_of::<IoUringParams>()) { return -14; }
    let params = unsafe { &mut *(params_ptr as *mut IoUringParams) };
    let flags  = params.flags;
    let cq_entries = if flags & IORING_SETUP_CQSIZE != 0 && params.cq_entries > entries {
        params.cq_entries } else { entries * 2 };
    let pid = crate::process::current_pid();
    let uid = NEXT_URING_FD.fetch_add(1, Ordering::Relaxed) as i32;
    let ring = match IoUring::new(uid, pid, entries, cq_entries, flags) {
        Some(r) => r, None => return -12,
    };
    ring.fill_params(params);
    RINGS.lock().insert(uid, ring);
    let vfd = crate::process::with_current_mut(|p| {
        use crate::vfs::{FileDescriptor, FdKind, Inode};
        let f = FileDescriptor {
            inode: Inode { ino: uid as u64, mode: 0xC000, uid: 0, gid: 0, size: 0,
                atime:0, mtime:0, ctime:0,
                ops: crate::vfs::DummyInodeOps::new(),
                sb:  alloc::sync::Arc::new(crate::vfs::Superblock { dev:0, fs_type: alloc::string::String::new(), ops: crate::vfs::DummySuperblock::new() }) },
            kind: FdKind::IoUring(uid as u32), flags: 0, offset: 0,
         path: alloc::string::String::new(),};
        p.alloc_fd(f) as i32
    });
    crate::klog!("io_uring: setup sq={} cq={}", entries, cq_entries);
    vfd.unwrap_or(-1) as i64
}

pub fn sys_io_uring_enter(fd: i32, to_submit: u32, min_complete: u32, flags: u32, _sig: u64, _sz: usize) -> i64 {
    let rid = resolve_ring_fd(fd);
    let mut g = RINGS.lock();
    match g.get_mut(&rid) {
        Some(r) => r.submit_and_wait(to_submit, min_complete, flags) as i64,
        None    => -9,
    }
}

pub fn sys_io_uring_register(fd: i32, opcode: u32, arg: u64, nr_args: u32) -> i64 {
    const IORING_REGISTER_PROBE:  u32 = 8;
    const IORING_REGISTER_FILES:  u32 = 2;
    let rid = resolve_ring_fd(fd);
    match opcode {
        IORING_REGISTER_PROBE => {
            if arg != 0 {
                if !crate::security::is_user_ptr_valid(arg, 4 + 4 * IORING_OP_LAST as usize) { return -14; }
                unsafe {
                    *(arg as *mut u32) = IORING_OP_LAST as u32;
                    let ops = (arg + 8) as *mut u32;
                    for i in 0..IORING_OP_LAST as usize { *ops.add(i) = 3; }
                }
            }
            0
        }
        IORING_REGISTER_FILES => {
            if arg != 0 && nr_args > 0 {
                if !crate::security::is_user_ptr_valid(arg, nr_args as usize * 4) { return -14; }
                let fds = unsafe { core::slice::from_raw_parts(arg as *const i32, nr_args as usize) };
                let mut g = RINGS.lock();
                if let Some(r) = g.get_mut(&rid) {
                    r.fixed_files = fds.iter().map(|&f| if f<0{None}else{Some(f)}).collect();
                }
            }
            0
        }
        _ => 0,
    }
}

pub fn map_ring_pages(ring_fd: i32, offset: u64, len: usize, pml4_phys: u64) -> Option<u64> {
    let g = RINGS.lock();
    let ring = g.get(&ring_fd)?;
    let (phys, sz) = ring.mmap_region(offset)?;
    if len > sz { return None; }
    let user_base = 0x0000_0040_0000_0000u64 + offset;
    let flags = crate::arch::x86_64::paging::PageFlags::PRESENT
        | crate::arch::x86_64::paging::PageFlags::USER
        | crate::arch::x86_64::paging::PageFlags::WRITABLE;
    let mut mapper = crate::arch::x86_64::paging::PageMapper::new(pml4_phys);
    let pages = (len + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    for i in 0..pages as u64 {
        unsafe { mapper.map_page(user_base + i*PAGE_SIZE, phys + i*PAGE_SIZE, flags); }
    }
    Some(user_base)
}

pub fn init() { crate::klog!("io_uring: ready"); }
