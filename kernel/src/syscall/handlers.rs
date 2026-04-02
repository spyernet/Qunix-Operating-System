use alloc::vec::Vec;
// All kernel syscall handler implementations.

use alloc::string::String;
use alloc::vec;
use alloc::sync::Arc;
use crate::process;
use crate::vfs::{self, O_CREAT, O_EXCL, O_TRUNC, O_APPEND, O_RDONLY, O_WRONLY, O_RDWR};
use crate::memory::vmm::Prot;
use crate::signal;
use crate::time::{Timespec, clock_gettime};

// Negative errno values
const EPERM:  i64 = -1;  const ENOENT: i64 = -2;  const ESRCH:  i64 = -3;
const EIO:    i64 = -5;  const EBADF:  i64 = -9;  const ECHILD: i64 = -10;
const EAGAIN: i64 = -11; const ENOMEM: i64 = -12; const EACCES: i64 = -13;
const EFAULT: i64 = -14; const EEXIST: i64 = -17; const EXDEV:  i64 = -18;
const ENOTDIR:i64 = -20; const EISDIR: i64 = -21; const EINVAL: i64 = -22;
const EMFILE: i64 = -24; const ENOSPC: i64 = -28; const EPIPE:  i64 = -32;
const ENOSYS: i64 = -38; const ENAMETOOLONG: i64 = -36;

// ── Helpers ───────────────────────────────────────────────────────────────

pub unsafe fn read_cstr(ptr: u64) -> Option<String> {
    if ptr == 0 { return None; }
    if !crate::security::is_user_ptr_valid(ptr, 1) { return None; }
    let mut s = String::new();
    let mut p = ptr as *const u8;
    loop {
        let c = *p;
        if c == 0 { break; }
        s.push(c as char);
        p = p.add(1);
        if s.len() > 4096 { return None; }
    }
    Some(s)
}

/// Read a NULL-terminated array of C string pointers from user space.
/// Validates every pointer slot before dereferencing.
/// Limits: max 4096 elements, max 2 MiB total string data.
pub unsafe fn read_cstr_array(ptr: u64) -> alloc::vec::Vec<String> {
    const MAX_ELEMS: usize = 4096;
    const MAX_TOTAL: usize = 2 * 1024 * 1024; // 2 MiB

    let mut v = alloc::vec::Vec::new();
    if ptr == 0 { return v; }

    let mut total_bytes = 0usize;

    for i in 0..MAX_ELEMS {
        // Compute address of this pointer slot and validate it
        let slot_addr = ptr + (i as u64) * 8;
        if !crate::security::is_user_ptr_valid(slot_addr, 8) { break; }

        let p = *(slot_addr as *const u64);
        if p == 0 { break; } // NULL terminator

        if let Some(s) = read_cstr(p) {
            total_bytes += s.len() + 1;
            if total_bytes > MAX_TOTAL { break; } // total size cap
            v.push(s);
        } else {
            // Invalid pointer in the array — stop here for safety
            break;
        }
    }
    v
}

fn vfs_err(e: crate::vfs::VfsError) -> i64 { -(e as i64) }

// ── File I/O ──────────────────────────────────────────────────────────────

pub fn sys_read(fd: i32, buf: u64, count: usize) -> i64 {
    if count == 0 { return 0; }
    // Validate user buffer pointer before touching it
    if buf == 0 || !crate::security::is_user_ptr_valid(buf, count) { return EFAULT; }
    let result = process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| {
            vfs::read_fd(&f, buf as *mut u8, count)
        })
    }).flatten();
    match result {
        Some(Ok(n))  => {
            // Advance offset
            process::with_current_mut(|p| {
                p.get_fd_mut_op(fd as u32, |f| {
                    match &f.kind {
                        crate::vfs::FdKind::Regular | crate::vfs::FdKind::Directory => {
                            f.offset += n as u64;
                        }
                        _ => {}
                    }
                });
            });
            n as i64
        }
        Some(Err(e)) => vfs_err(e),
        None         => EBADF,
    }
}

pub fn sys_write(fd: i32, buf: u64, count: usize) -> i64 {
    if count == 0 { return 0; }
    // Validate user buffer pointer before touching it
    if buf == 0 || !crate::security::is_user_ptr_valid(buf, count) { return EFAULT; }
    let result = process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| {
            vfs::write_fd(&f, buf as *const u8, count)
        })
    }).flatten();
    match result {
        Some(Ok(n))  => {
            process::with_current_mut(|p| {
                p.get_fd_mut_op(fd as u32, |f| {
                    if let crate::vfs::FdKind::Regular = &f.kind { f.offset += n as u64; }
                });
            });
            n as i64
        }
        Some(Err(e)) => vfs_err(e),
        None         => EBADF,
    }
}

pub fn sys_pread64(fd: i32, buf: u64, count: usize, offset: i64) -> i64 {
    let result = process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| {
            let mut f2 = f.clone();
            f2.offset = offset as u64;
            vfs::read_fd(&f2, buf as *mut u8, count)
        })
    }).flatten();
    match result { Some(Ok(n)) => n as i64, Some(Err(e)) => vfs_err(e), None => EBADF }
}

pub fn sys_pwrite64(fd: i32, buf: u64, count: usize, offset: i64) -> i64 {
    let result = process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| {
            let mut f2 = f.clone();
            f2.offset = offset as u64;
            vfs::write_fd(&f2, buf as *const u8, count)
        })
    }).flatten();
    match result { Some(Ok(n)) => n as i64, Some(Err(e)) => vfs_err(e), None => EBADF }
}

#[repr(C)] struct IoVec { base: u64, len: u64 }

pub fn sys_readv(fd: i32, iov: u64, iovcnt: usize) -> i64 {
    let mut total = 0i64;
    for i in 0..iovcnt {
        let v = unsafe { &*((iov + i as u64 * 16) as *const IoVec) };
        if v.len == 0 { continue; }
        let r = sys_read(fd, v.base, v.len as usize);
        if r < 0 { return if total == 0 { r } else { total }; }
        total += r;
    }
    total
}

pub fn sys_writev(fd: i32, iov: u64, iovcnt: usize) -> i64 {
    let mut total = 0i64;
    for i in 0..iovcnt {
        let v = unsafe { &*((iov + i as u64 * 16) as *const IoVec) };
        if v.len == 0 { continue; }
        let r = sys_write(fd, v.base, v.len as usize);
        if r < 0 { return if total == 0 { r } else { total }; }
        total += r;
    }
    total
}

pub fn sys_open(path: u64, flags: i32, mode: u32) -> i64 {
    let pathname = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd      = process::with_current(|p| p.get_cwd()).unwrap_or_default();
    match vfs::open(&cwd, &pathname, flags, mode) {
        Ok(fd) => process::with_current_mut(|p| p.alloc_fd(fd) as i64).unwrap_or(EBADF),
        Err(e) => vfs_err(e),
    }
}

pub fn sys_openat(dirfd: i32, path: u64, flags: i32, mode: u32) -> i64 {
    const AT_FDCWD: i32 = -100;
    let pathname = match unsafe { read_cstr(path) } { Some(s) => s, None => return EFAULT };

    // Absolute paths and AT_FDCWD use the process cwd; no dirfd needed.
    if pathname.starts_with('/') || dirfd == AT_FDCWD {
        let cwd = process::with_current(|p| p.get_cwd()).unwrap_or_default();
        return match vfs::open(&cwd, &pathname, flags, mode) {
            Ok(fd_obj) => process::with_current_mut(|p| p.alloc_fd(fd_obj) as i64).unwrap_or(EBADF),
            Err(e)     => vfs_err(e),
        };
    }

    // Relative path: resolve relative to the directory referenced by dirfd.
    let base_path = process::with_current(|p| {
        p.get_fd(dirfd as u32).and_then(|f| {
            // Only directory fds are valid bases for relative openat.
            if vfs::s_isdir(f.inode.mode) {
                // Use the stored absolute path of this fd.
                let p = f.path.clone();
                if p.is_empty() { None } else { Some(p) }
            } else {
                None
            }
        })
    }).flatten();

    let base = match base_path {
        Some(p) => p,
        None    => return EBADF,
    };

    match vfs::open(&base, &pathname, flags, mode) {
        Ok(fd_obj) => process::with_current_mut(|p| p.alloc_fd(fd_obj) as i64).unwrap_or(EBADF),
        Err(e)     => vfs_err(e),
    }
}

pub fn sys_close(fd: i32) -> i64 {
    // Clone the kind before removing so we can call close/wake after
    let kind = process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| f.kind.clone())
    }).flatten();

    let closed = process::with_current_mut(|p| p.close_fd(fd as u32));
    if !closed.unwrap_or(false) { return EBADF; }

    // Decrement pipe reference counts and wake blocked peers.
    // Must happen AFTER removing the fd from the table so that the
    // Arc refcount drop reflects the actual remaining holders.
    match kind {
        Some(crate::vfs::FdKind::PipeWrite(pipe)) => {
            crate::ipc::pipe::pipe_close_write(&pipe);
        }
        Some(crate::vfs::FdKind::PipeRead(pipe)) => {
            crate::ipc::pipe::pipe_close_read(&pipe);
        }
        _ => {}
    }
    0
}

pub fn sys_lseek(fd: i32, offset: i64, whence: i32) -> i64 {
    // SEEK_END (whence=2): must use the *live* file size, not the cached inode.size.
    // Re-stat via ops to get the current size, then update the fd snapshot.
    if whence == 2 {
        // Refresh inode size before delegating to vfs::lseek
        let refresh = process::with_current(|p| {
            p.get_fd(fd as u32).map(|f| {
                // Ask the inode ops for current stat to get live size
                let live_size = f.inode.ops.read(&f.inode, &mut [], 0)
                    .ok()
                    .map(|_| f.inode.size) // read(0 bytes) just probes
                    .unwrap_or(f.inode.size);
                live_size
            })
        });
        if let Some(live_size) = refresh.flatten() {
            // Update cached inode size on the fd
            process::with_current_mut(|p| {
                p.get_fd_mut_op(fd as u32, |f| { f.inode.size = live_size; });
            });
        }
    }
    let r = process::with_current_mut(|p| {
        p.get_fd_mut_op(fd as u32, |f| vfs::lseek(f, offset, whence))
    }).flatten();
    match r { Some(Ok(pos)) => pos as i64, Some(Err(e)) => vfs_err(e), None => EBADF }
}

pub fn sys_stat(path: u64, buf: u64) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::stat(&cwd, &p) {
        Ok(s) => { unsafe { *(buf as *mut vfs::Stat) = s; } 0 }
        Err(e) => vfs_err(e),
    }
}

pub fn sys_lstat(path: u64, buf: u64) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::lstat(&cwd, &p) {
        Ok(s) => { unsafe { *(buf as *mut vfs::Stat) = s; } 0 }
        Err(e) => vfs_err(e),
    }
}

pub fn sys_fstat(fd: i32, buf: u64) -> i64 {
    let r = process::with_current(|p| p.get_fd(fd as u32).map(|f| vfs::fstat(&f))).flatten();
    match r { Some(Ok(s)) => { unsafe { *(buf as *mut vfs::Stat) = s; } 0 }
              Some(Err(e)) => vfs_err(e), None => EBADF }
}

pub fn sys_newfstatat(dirfd: i32, path: u64, buf: u64, flags: i32) -> i64 {
    if flags & 0x100 != 0 { // AT_SYMLINK_NOFOLLOW
        sys_lstat(path, buf)
    } else {
        sys_stat(path, buf)
    }
}

pub fn sys_access(path: u64, mode: i32) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::stat(&cwd, &p) {
        Ok(_)  => 0,
        Err(e) => vfs_err(e),
    }
}

pub fn sys_faccessat(dirfd: i32, path: u64, mode: i32, flags: i32) -> i64 {
    sys_access(path, mode)
}

pub fn sys_truncate(path: u64, len: i64) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::open(&cwd, &p, O_WRONLY, 0) {
        Ok(fd) => match fd.inode.ops.truncate(&fd.inode, len as u64) { Ok(_)=>0, Err(e)=>vfs_err(e) }
        Err(e) => vfs_err(e),
    }
}

pub fn sys_ftruncate(fd: i32, len: i64) -> i64 {
    let r = process::with_current(|p| p.get_fd(fd as u32).map(|f| f.inode.ops.truncate(&f.inode, len as u64))).flatten();
    match r { Some(Ok(_))=>0, Some(Err(e))=>vfs_err(e), None=>EBADF }
}

pub fn sys_getdents64(fd: i32, buf: u64, count: usize) -> i64 {
    // Use getdents_and_advance so we get (bytes_written, entries_consumed).
    // fd.offset is an entry count: we skip that many entries, then advance it.
    let r = process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| {
            vfs::getdents_and_advance(&f, buf as *mut u8, count)
        })
    }).flatten();
    match r {
        Some(Ok((bytes, entries))) => {
            if entries > 0 {
                // Advance offset by the number of directory entries consumed.
                process::with_current_mut(|p| {
                    p.get_fd_mut_op(fd as u32, |f| { f.offset += entries; });
                });
            }
            bytes as i64
        }
        Some(Err(e)) => vfs_err(e),
        None => EBADF,
    }
}

/// Old-style getdents(2) syscall (nr=78).
/// Same as getdents64 but historically used a different struct layout.
/// Modern kernels map it to the same implementation; we do the same.
pub fn sys_getdents(fd: i32, buf: u64, count: u32) -> i64 {
    sys_getdents64(fd, buf, count as usize)
}

pub fn sys_getcwd(buf: u64, size: usize) -> i64 {
    let cwd = process::with_current(|p| p.get_cwd()).unwrap_or_default();
    let b   = cwd.as_bytes();
    if b.len() + 1 > size { return -(vfs::ENAMETOOLONG as i64); }
    unsafe {
        core::ptr::copy_nonoverlapping(b.as_ptr(), buf as *mut u8, b.len());
        *((buf + b.len() as u64) as *mut u8) = 0;
    }
    buf as i64
}

pub fn sys_chdir(path: u64) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::resolve_dir(&cwd, &p) {
        Ok(new_cwd) => { process::with_current_mut(|proc| proc.set_cwd(new_cwd)); 0 }
        Err(e)      => vfs_err(e),
    }
}

pub fn sys_fchdir(fd: i32) -> i64 {
    let inode = process::with_current(|p| p.get_fd(fd as u32).map(|f| (f.inode.clone(), f.inode.mode)));
    match inode.flatten() {
        Some((ino, mode)) => {
            if !vfs::s_isdir(mode) { return -(vfs::ENOTDIR as i64); }
            // TODO: reconstruct path from inode
            0
        }
        None => EBADF,
    }
}

pub fn sys_mkdir(path: u64, mode: u32) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::mkdir(&cwd, &p, mode) { Ok(_)=>0, Err(e)=>vfs_err(e) }
}

pub fn sys_mkdirat(dirfd: i32, path: u64, mode: u32) -> i64 { sys_mkdir(path, mode) }

pub fn sys_rmdir(path: u64) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::rmdir(&cwd, &p) { Ok(_)=>0, Err(e)=>vfs_err(e) }
}

pub fn sys_unlink(path: u64) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::unlink(&cwd, &p) { Ok(_)=>0, Err(e)=>vfs_err(e) }
}

pub fn sys_unlinkat(dirfd: i32, path: u64, flags: i32) -> i64 {
    if flags & 0x200 != 0 { sys_rmdir(path) } else { sys_unlink(path) }
}

pub fn sys_symlink(target: u64, linkpath: u64) -> i64 {
    let tgt  = match unsafe { read_cstr(target)   } { Some(s)=>s, None=>return EFAULT };
    let link = match unsafe { read_cstr(linkpath) } { Some(s)=>s, None=>return EFAULT };
    let cwd  = process::with_current(|p| p.get_cwd()).unwrap_or_default();
    match vfs::symlink(&cwd, &tgt, &link) { Ok(_)=>0, Err(e)=>vfs_err(e) }
}

pub fn sys_symlinkat(target: u64, _dirfd: i32, linkpath: u64) -> i64 {
    sys_symlink(target, linkpath)
}

pub fn sys_readlink(path: u64, buf: u64, size: usize) -> i64 {
    let p = match unsafe { read_cstr(path) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|proc| proc.get_cwd()).unwrap_or_default();
    match vfs::readlink(&cwd, &p) {
        Ok(target) => {
            let b = target.as_bytes();
            let n = b.len().min(size);
            unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), buf as *mut u8, n); }
            n as i64
        }
        Err(e) => vfs_err(e),
    }
}

pub fn sys_rename(old: u64, new: u64) -> i64 {
    let o = match unsafe { read_cstr(old) } { Some(s)=>s, None=>return EFAULT };
    let n = match unsafe { read_cstr(new) } { Some(s)=>s, None=>return EFAULT };
    let cwd = process::with_current(|p| p.get_cwd()).unwrap_or_default();
    match vfs::rename(&cwd, &o, &n) { Ok(_)=>0, Err(e)=>vfs_err(e) }
}

pub fn sys_renameat(_od: i32, old: u64, _nd: i32, new: u64) -> i64 { sys_rename(old, new) }
pub fn sys_renameat2(_od: i32, old: u64, _nd: i32, new: u64, _f: u32) -> i64 { sys_rename(old, new) }

pub fn sys_link(old: u64, new: u64) -> i64 { -(vfs::ENOSYS as i64) } // Hardlinks not implemented

pub fn sys_chmod(path: u64, mode: u32) -> i64 { 0 }
pub fn sys_fchmod(fd: i32, mode: u32) -> i64 { 0 }
pub fn sys_fchmodat(_dirfd: i32, path: u64, mode: u32, _flags: i32) -> i64 { 0 }

pub fn sys_chown(path: u64, uid: u32, gid: u32) -> i64 { 0 }
pub fn sys_fchown(fd: i32, uid: u32, gid: u32) -> i64 { 0 }
pub fn sys_lchown(path: u64, uid: u32, gid: u32) -> i64 { 0 }
pub fn sys_fchownat(_df: i32, path: u64, uid: u32, gid: u32, _f: i32) -> i64 { 0 }

/// Increment the pipe reference count when an fd holding a pipe end is dup'd.
/// Must be called after the new fd is installed in the fd table.
fn pipe_dup_inc(kind: &crate::vfs::FdKind) {
    match kind {
        crate::vfs::FdKind::PipeWrite(pipe) => { pipe.lock().dup_write(); }
        crate::vfs::FdKind::PipeRead(pipe)  => { pipe.lock().dup_read();  }
        _ => {}
    }
}

pub fn sys_dup(fd: i32) -> i64 {
    let (new_fd_num, kind) = match process::with_current_mut(|p| {
        p.get_fd(fd as u32).map(|f| {
            let k = f.kind.clone();
            let c = f.clone();
            let n = p.alloc_fd(c);
            (n, k)
        })
    }).flatten() {
        Some(v) => v,
        None    => return EBADF,
    };
    // Increment pipe refcount so close(original) doesn't prematurely EOF
    pipe_dup_inc(&kind);
    new_fd_num as i64
}

pub fn sys_dup2(old: i32, new: i32) -> i64 {
    if old == new {
        return if process::with_current(|p| p.get_fd(old as u32).is_some())
                   .unwrap_or(false) { new as i64 } else { EBADF };
    }

    // Snapshot the source fd and any existing fd at new
    let (src_fd, old_new_kind) = match process::with_current(|p| {
        let src = p.get_fd(old as u32)?;
        let old_new = p.get_fd(new as u32).map(|f| f.kind.clone());
        Some((src, old_new))
    }).flatten() {
        Some(v) => v,
        None    => return EBADF,
    };

    // Close the fd currently at `new` (with proper pipe reference handling)
    if let Some(kind) = old_new_kind {
        match kind {
            crate::vfs::FdKind::PipeWrite(pipe) => {
                crate::ipc::pipe::pipe_close_write(&pipe);
            }
            crate::vfs::FdKind::PipeRead(pipe) => {
                crate::ipc::pipe::pipe_close_read(&pipe);
            }
            _ => {}
        }
    }

    // Install src at new
    let new_kind = src_fd.kind.clone();
    process::with_current_mut(|p| p.alloc_fd_at(new as u32, src_fd));

    // Increment pipe refcount for the new copy
    pipe_dup_inc(&new_kind);

    new as i64
}

pub fn sys_dup3(old: i32, new: i32, flags: i32) -> i64 {
    if old == new { return EINVAL; }
    let r = sys_dup2(old, new);
    if r >= 0 && flags & crate::vfs::O_CLOEXEC != 0 {
        // Set O_CLOEXEC on the new fd
        process::with_current_mut(|p| {
            p.get_fd_mut_op(new as u32, |f| { f.flags |= crate::vfs::O_CLOEXEC; });
        });
    }
    r
}

pub fn sys_pipe(fds: u64) -> i64 {
    if fds == 0 || fds >= 0x0000_8000_0000_0000 { return EFAULT; }
    let (r, w) = crate::ipc::pipe::new_pipe();
    let (rfd, wfd) = process::with_current_mut(|p| (p.alloc_fd(r), p.alloc_fd(w))).unwrap_or((0, 0));
    // Write two ints (4 bytes each) to user memory
    unsafe {
        *(fds       as *mut i32) = rfd as i32;
        *((fds + 4) as *mut i32) = wfd as i32;
    }
    0
}

pub fn sys_pipe2(fds: u64, flags: i32) -> i64 {
    if fds == 0 || fds >= 0x0000_8000_0000_0000 { return EFAULT; }
    let (r, w) = crate::ipc::pipe::new_pipe2(flags);
    let (rfd, wfd) = process::with_current_mut(|p| (p.alloc_fd(r), p.alloc_fd(w))).unwrap_or((0, 0));
    unsafe {
        *(fds       as *mut i32) = rfd as i32;
        *((fds + 4) as *mut i32) = wfd as i32;
    }
    0
}

pub fn sys_fcntl(fd: i32, cmd: i32, arg: u64) -> i64 {
    const F_DUPFD: i32 = 0; const F_GETFD: i32 = 1; const F_SETFD: i32 = 2;
    const F_GETFL: i32 = 3; const F_SETFL: i32 = 4; const F_SETLK: i32 = 6;
    const F_GETLK: i32 = 5; const F_SETLKW: i32 = 7; const F_SETOWN: i32 = 8;
    const F_GETOWN: i32 = 9; const F_DUPFD_CLOEXEC: i32 = 1030;

    match cmd {
        F_DUPFD | F_DUPFD_CLOEXEC => {
            let (new_num, kind) = match process::with_current_mut(|p| {
                p.get_fd(fd as u32).map(|f| {
                    let k = f.kind.clone();
                    let c = f.clone();
                    let n = p.alloc_fd(c);
                    (n, k)
                })
            }).flatten() {
                Some(v) => v,
                None    => return EBADF,
            };
            pipe_dup_inc(&kind);
            if cmd == 1030 { // F_DUPFD_CLOEXEC
                process::with_current_mut(|p| {
                    p.get_fd_mut_op(new_num, |f| { f.flags |= vfs::O_CLOEXEC; });
                });
            }
            new_num as i64
        }
        F_GETFD => {
            let flags = process::with_current(|p| p.get_fd(fd as u32).map(|f| f.flags)).flatten();
            match flags { Some(f) => (f & vfs::O_CLOEXEC) as i64, None => EBADF }
        }
        F_SETFD => { process::with_current_mut(|p| { p.get_fd_mut_op(fd as u32, |f| { if arg & 1 != 0 { f.flags |= vfs::O_CLOEXEC; } else { f.flags &= !vfs::O_CLOEXEC; } }); }); 0 }
        F_GETFL => { process::with_current(|p| p.get_fd(fd as u32).map(|f| f.flags as i64)).flatten().unwrap_or(EBADF) }
        F_SETFL => { process::with_current_mut(|p| { p.get_fd_mut_op(fd as u32, |f| { f.flags = (f.flags & !0o10000777) | (arg as i32 & 0o10000777); }); }); 0 }
        F_GETLK | F_SETLK | F_SETLKW => 0,
        F_SETOWN | F_GETOWN => 0,
        _ => EINVAL,
    }
}

pub fn sys_fsync(fd: i32) -> i64 {
    let r = process::with_current(|p| p.get_fd(fd as u32).map(|f| f.inode.ops.fsync(&f.inode))).flatten();
    match r { Some(Ok(_))=>0, Some(Err(e))=>vfs_err(e), None=>EBADF }
}

pub fn sys_fdatasync(fd: i32) -> i64 { sys_fsync(fd) }

pub fn sys_sync() -> i64 {
    // Sync all mounted filesystems
    let mounts: alloc::vec::Vec<alloc::sync::Arc<vfs::Superblock>> = {
        let m = crate::vfs::MOUNTS.lock();
        m.iter().map(|mp| mp.sb.clone()).collect()
    };
    for sb in mounts { sb.ops.sync(); }
    0
}

pub fn sys_statfs(path: u64, buf: u64) -> i64 {
    if buf == 0 { return EFAULT; }
    let sf = unsafe { &mut *(buf as *mut vfs::StatFs) };
    let p  = process::with_current(|p| p.get_cwd()).unwrap_or_default();
    sf.f_type    = 0x01021994; // tmpfs
    sf.f_bsize   = 4096;
    sf.f_blocks  = crate::memory::phys::total_frames() as u64;
    sf.f_bfree   = crate::memory::phys::free_frames() as u64;
    sf.f_bavail  = sf.f_bfree;
    sf.f_namelen = 255;
    sf.f_frsize  = 4096;
    0
}

pub fn sys_fstatfs(fd: i32, buf: u64) -> i64 { sys_statfs(0, buf) }

// ── Memory ────────────────────────────────────────────────────────────────

pub fn sys_mmap(addr: u64, len: u64, prot: i32, flags: i32, fd: i32, off: u64) -> i64 {
    use crate::memory::vmm::{MAP_SHARED, MAP_PRIVATE, MAP_FIXED, MAP_ANONYMOUS};

    if len == 0 { return EINVAL; }

    let p        = Prot::from_bits_truncate(prot as u32);
    let map_flags = flags as u32;

    // ── DRM special case ────────────────────────────────────────────────
    if fd >= 0 && map_flags & MAP_SHARED != 0 && off != 0 {
        let is_drm = process::with_current(|proc| {
            proc.get_fd(fd as u32)
                .map(|f| matches!(f.kind, crate::vfs::FdKind::Drm))
                .unwrap_or(false)
        }).unwrap_or(false);
        if is_drm {
            return match crate::drm::mmap_gem(off, len, p) {
                Some(v) => v as i64,
                None    => ENOMEM,
            };
        }
    }

    let hint = if addr != 0 { Some(addr) } else { None };

    // ── Anonymous mapping ───────────────────────────────────────────────
    if map_flags & MAP_ANONYMOUS != 0 || fd < 0 {
        return match process::with_current_mut(|proc| {
            proc.address_space.mmap_full(hint, len, p, map_flags, None, 0)
        }).flatten() {
            Some(ptr) => ptr as i64,
            None      => ENOMEM,
        };
    }

    // ── File-backed mapping ─────────────────────────────────────────────
    // Read the file data from offset `off` into kernel memory, then
    // populate the mapping with that data.
    let file_data: alloc::vec::Vec<u8> = {
        let fd_clone = match process::with_current(|proc| proc.get_fd(fd as u32)).flatten() {
            Some(f) => f,
            None    => return EBADF,
        };

        // Validate: must be a regular file
        if !matches!(fd_clone.kind, crate::vfs::FdKind::Regular) {
            // Devices/pipes: treat as anonymous with no data
            return match process::with_current_mut(|proc| {
                proc.address_space.mmap_full(hint, len, p, map_flags | MAP_ANONYMOUS, None, 0)
            }).flatten() {
                Some(ptr) => ptr as i64,
                None      => ENOMEM,
            };
        }

        // Read `len` bytes starting at `off`
        let read_len = len as usize;
        let mut buf  = alloc::vec![0u8; read_len];
        let n = fd_clone.inode.ops.read(&fd_clone.inode, &mut buf, off)
            .unwrap_or(0);
        buf.truncate(n);
        buf
    };

    // Map the file data — pass slice to mmap_full for page population
    match process::with_current_mut(|proc| {
        proc.address_space.mmap_full(hint, len, p, map_flags, Some(&file_data), off)
    }).flatten() {
        Some(ptr) => ptr as i64,
        None      => ENOMEM,
    }
}

pub fn sys_munmap(addr: u64, len: u64) -> i64 {
    process::with_current_mut(|p| p.address_space.munmap(addr, len)); 0
}

pub fn sys_mprotect(addr: u64, len: u64, prot: i32) -> i64 {
    let p = Prot::from_bits_truncate(prot as u32);
    process::with_current_mut(|proc| proc.address_space.mprotect(addr, len, p));
    0
}

pub fn sys_madvise(_addr: u64, _len: u64, _advice: i32) -> i64 { 0 }
pub fn sys_msync(_addr: u64, _len: u64, _flags: i32) -> i64 { 0 }
pub fn sys_mlock(_addr: u64, _len: u64) -> i64 { 0 }
pub fn sys_munlock(_addr: u64, _len: u64) -> i64 { 0 }

pub fn sys_brk(new_brk: u64) -> i64 {
    process::with_current_mut(|p| {
        if new_brk == 0 { p.address_space.brk as i64 }
        else            { p.address_space.set_brk(new_brk) as i64 }
    }).unwrap_or(0)
}

// ── Process ───────────────────────────────────────────────────────────────

pub fn sys_getpid()  -> i64 { process::current_pid() as i64 }
pub fn sys_getppid() -> i64 { process::with_current(|p| p.ppid as i64).unwrap_or(0) }
pub fn sys_getpgrp() -> i64 { process::with_current(|p| p.pgid).unwrap_or(0) as i64 }
pub fn sys_getpgid(pid: i32) -> i64 {
    let target = if pid == 0 { process::current_pid() } else { pid as u32 };
    process::with_process(target, |p| p.pgid).unwrap_or(0) as i64
}
pub fn sys_setpgid(pid: i32, pgid: i32) -> i64 {
    let target = if pid == 0 { process::current_pid() } else { pid as u32 };
    let pg     = if pgid == 0 { target } else { pgid as u32 };

    // POSIX: a process that is a session leader cannot change its pgid.
    let is_session_leader = process::with_process(target, |p| p.pid == p.sid)
        .unwrap_or(false);
    if is_session_leader { return EPERM; }

    // POSIX: can only move into a process group in the same session.
    let (target_sid, current_sid) = (
        process::with_process(target, |p| p.sid).unwrap_or(0),
        process::with_current(|p| p.sid).unwrap_or(0),
    );
    if target_sid != current_sid { return EPERM; }

    process::with_process_mut(target, |p| p.pgid = pg);
    0
}
pub fn sys_getsid(pid: i32) -> i64 {
    let target = if pid == 0 { process::current_pid() } else { pid as u32 };
    process::with_process(target, |p| p.sid).unwrap_or(0) as i64
}
pub fn sys_setsid() -> i64 {
    let pid = process::current_pid();
    // POSIX: cannot call setsid if already a process group leader
    let is_leader = process::with_current(|p| p.pid == p.pgid).unwrap_or(false);
    if is_leader { return EPERM; }

    process::with_current_mut(|p| {
        p.sid  = pid;
        p.pgid = pid;
        p.tty  = -1; // no controlling terminal yet
    }); // returns Option<()>, discarded
    pid as i64
}
pub fn sys_getuid() -> i64 {
    let (uid, user_ns) = process::with_current(|p| (p.uid, p.namespaces.user_ns))
        .unwrap_or((0, 0));
    if user_ns == 0 { uid as i64 }
    else { crate::security::namespace::map_uid_from_host(user_ns, uid) as i64 }
}
pub fn sys_getgid() -> i64 {
    let (gid, user_ns) = process::with_current(|p| (p.gid, p.namespaces.user_ns))
        .unwrap_or((0, 0));
    if user_ns == 0 { gid as i64 }
    else { crate::security::namespace::map_gid_to_host(user_ns, gid) as i64 }
}
pub fn sys_geteuid() -> i64 { process::with_current(|p| p.euid as i64).unwrap_or(0) }
pub fn sys_getegid() -> i64 { process::with_current(|p| p.egid as i64).unwrap_or(0) }
pub fn sys_setuid(uid: u32) -> i64 {
    process::with_current_mut(|p| { p.uid = uid; p.euid = uid; p.suid = uid; });
    0
}
pub fn sys_setgid(gid: u32) -> i64 {
    process::with_current_mut(|p| { p.gid = gid; p.egid = gid; p.sgid = gid; });
    0
}
pub fn sys_setresuid(ruid: u32, euid: u32, suid: u32) -> i64 {
    process::with_current_mut(|p| { if ruid != u32::MAX { p.uid = ruid; } if euid != u32::MAX { p.euid = euid; } if suid != u32::MAX { p.suid = suid; } });
    0
}
pub fn sys_setresgid(rgid: u32, egid: u32, sgid: u32) -> i64 {
    process::with_current_mut(|p| { if rgid != u32::MAX { p.gid = rgid; } if egid != u32::MAX { p.egid = egid; } if sgid != u32::MAX { p.sgid = sgid; } });
    0
}
pub fn sys_getgroups(size: i32, list: u64) -> i64 {
    let groups: alloc::vec::Vec<u32> = process::with_current(|p| p.groups.clone()).unwrap_or_default();
    if size == 0 { return groups.len() as i64; }
    let n = groups.len().min(size as usize);
    for i in 0..n { unsafe { *((list + i as u64 * 4) as *mut u32) = groups[i]; } }
    n as i64
}
pub fn sys_setgroups(size: i32, list: u64) -> i64 {
    let mut gs = alloc::vec::Vec::new();
    for i in 0..size as usize { gs.push(unsafe { *((list + i as u64 * 4) as *const u32) }); }
    process::with_current_mut(|p| p.groups = gs); 0
}

pub fn sys_umask(mask: u32) -> i64 {
    process::with_current_mut(|p| { let old = p.umask; p.umask = mask & 0o777; old })
        .unwrap_or(0o022) as i64
}

pub fn sys_fork() -> i64 {
    crate::perf::PERF.inc_ctx_switch();
    crate::drivers::serial::write_str("[sys_fork] enter\n");
    match process::fork_current() {
        Some(child) => {
            crate::drivers::serial::write_str("[sys_fork] fork_current ok\n");
            // Child gets return value 0; we return child pid to parent.
            // add_task enqueues child as Runnable.
            crate::sched::add_task(child, crate::sched::PRIO_NORMAL, crate::sched::SCHED_NORMAL);
            crate::drivers::serial::write_str("[sys_fork] add_task ok\n");
            // Signal the scheduler that a reschedule is needed so the child
            // gets CPU time at the next safe preemption point (syscall exit or
            // timer IRQ) instead of waiting for the full next tick.
            crate::sched::request_reschedule();
            child as i64
        }
        None => {
            crate::drivers::serial::write_str("[sys_fork] fork_current failed\n");
            ENOMEM
        }
    }
}

pub fn sys_vfork() -> i64 { sys_fork() }

pub fn sys_clone(flags: u64, stack: u64, ptid: u64, ctid: u64, tls: u64) -> i64 {
    crate::abi_compat::syscall::sys_clone(flags, stack, ptid, ctid, tls)
}

pub fn sys_execve(path: u64, argv: u64, envp: u64) -> i64 {
    let pathname = match unsafe { read_cstr(path) }  { Some(s)=>s, None=>return EFAULT };
    let args     = unsafe { read_cstr_array(argv) };
    let env      = unsafe { read_cstr_array(envp) };

    let cwd = process::with_current(|p| p.get_cwd()).unwrap_or_default();
    let fd  = match vfs::open(&cwd, &pathname, O_RDONLY, 0) { Ok(f)=>f, Err(e)=>return vfs_err(e) };
    // Reject directories immediately
    if vfs::s_isdir(fd.inode.mode) { return vfs_err(vfs::EISDIR); }
    // Reject zero-size files (permission denied is a better signal than ENOENT)
    if fd.inode.size == 0 { return vfs_err(vfs::EACCES); }
    let mut data = alloc::vec![];
    vfs::read_all_fd(&fd, &mut data);
    if data.is_empty() { return vfs_err(vfs::ENOENT); }

    match crate::elf::exec(data, args, env) {
        Ok(result) => {
            // Set FS.base for TLS if present
            if result.tls_addr != 0 {
                unsafe { crate::arch::x86_64::msr::write(
                    crate::arch::x86_64::msr::IA32_FSBASE, result.tls_addr); }
            }

            process::with_current_mut(|proc| {
                // ── 1. Replace address space ─────────────────────────────
                // Old address space is dropped here; page frames freed by Drop impl.
                proc.address_space = result.address_space;

                // ── 2. Reset execution context ───────────────────────────
                // context.rip/rsp are the KERNEL callee-saved register state
                // used by context_switch.  They are NOT the user-space RIP/RSP
                // at entry; those are provided directly to exec_usermode_noreturn
                // below.  We still update rip so /proc/pid/status shows a
                // reasonable value; rsp is left at 0 since the user stack is
                // established by the iretq, not context_switch.
                proc.context.rip   = result.entry;
                proc.name          = pathname.clone();
                proc.fs_base       = result.tls_addr;

                // ── 3. Close O_CLOEXEC fds (check both direct fds and thread group) ─
                // Collect keys first to avoid borrow issues
                let cloexec_direct: alloc::vec::Vec<u32> = proc.fds.iter()
                    .filter(|(_, f)| f.flags & vfs::O_CLOEXEC != 0)
                    .map(|(&k, _)| k)
                    .collect();
                for k in cloexec_direct { proc.fds.remove(&k); }

                if let Some(ref tg) = proc.thread_group.clone() {
                    let mut fds = tg.fds.lock();
                    let cloexec_tg: alloc::vec::Vec<u32> = fds.iter()
                        .filter(|(_, f)| f.flags & vfs::O_CLOEXEC != 0)
                        .map(|(&k, _)| k)
                        .collect();
                    for k in cloexec_tg { fds.remove(&k); }
                }

                // ── 4. Guarantee stdin/stdout/stderr ─────────────────────
                // If exec closed fd 0/1/2 (e.g. they had O_CLOEXEC), reopen /dev/null
                // so programs that assume these are valid don't crash.
                fn ensure_std_fd(
                    proc: &mut crate::process::Process,
                    fd_num: u32,
                ) {
                    if proc.get_fd(fd_num).is_some() { return; }
                    let cwd = alloc::string::String::from("/");
                    if let Ok(null_fd) = crate::vfs::open(
                        &cwd, "/dev/null", crate::vfs::O_RDWR, 0)
                    {
                        proc.alloc_fd_at(fd_num, null_fd);
                    }
                }
                ensure_std_fd(proc, 0);
                ensure_std_fd(proc, 1);
                ensure_std_fd(proc, 2);

                // ── 5. Reset signal handlers to default on exec ──────────
                // Per POSIX: caught signals reset to SIG_DFL; ignored signals stay ignored.
                for action in proc.sig_actions.iter_mut() {
                    if let crate::signal::SigHandler::User(_) = action.handler {
                        action.handler = crate::signal::SigHandler::Default;
                    }
                }
                proc.sig_pending = crate::signal::SignalSet::empty();
            });

            // Activate the new page tables before jumping
            process::with_current(|p| p.address_space.activate());
            unsafe { crate::elf::exec_usermode_noreturn(result.entry, result.stack_top) }
        }
        Err(e) => -(e as i64),
    }
}

pub fn sys_exit(code: i32) -> i64 {
    process::exit_current(code);
    let pid = process::current_pid();
    crate::sched::remove_task(pid);
    crate::sched::schedule_next_from_irq();
    unreachable!("sys_exit returned")
}

pub fn sys_exit_group(code: i32) -> i64 { sys_exit(code) }

pub fn sys_wait4(pid: i32, wstatus: u64, options: i32, rusage: u64) -> i64 {
    const WNOHANG: i32 = 1;
    const WUNTRACED: i32 = 2;

    let me = process::current_pid();

    // Helper: find a zombie child matching the pid spec.
    // Returns Some(child_pid) if found, None if no matching zombie exists.
    let find_zombie = || -> Option<process::Pid> {
        if pid == -1 {
            process::wait_any_zombie(me)
        } else if pid < -1 {
            let pgid = (-pid) as u32;
            process::all_pids().into_iter().find(|&p| {
                process::with_process(p, |proc| proc.pgid == pgid && proc.is_zombie())
                    .unwrap_or(false)
            })
        } else {
            let t = pid as u32;
            if process::with_process(t, |p| p.is_zombie()).unwrap_or(false) {
                Some(t)
            } else {
                None
            }
        }
    };

    // Helper: does the pid spec have any living or zombie children?
    // Used to distinguish ECHILD from "not yet exited".
    let has_relevant_children = || -> bool {
        if pid == -1 {
            process::with_current(|p| !p.children.is_empty()).unwrap_or(false)
        } else if pid < -1 {
            let pgid = (-pid) as u32;
            process::all_pids().into_iter().any(|p| {
                process::with_process(p, |proc| proc.pgid == pgid).unwrap_or(false)
            })
        } else {
            process::with_process(pid as u32, |_| ()).is_some()
        }
    };

    // First check: maybe a zombie is already available
    if let Some(cpid) = find_zombie() {
        let code = process::reap_child(cpid).unwrap_or(0);
        if wstatus != 0 {
            // POSIX exit status encoding: (exit_code & 0xFF) << 8
            unsafe { *(wstatus as *mut i32) = (code & 0xFF) << 8; }
        }
        return cpid as i64;
    }

    // WNOHANG: return 0 immediately if no zombie found
    if options & WNOHANG != 0 { return 0; }

    // No zombie yet. Check whether there are any children at all.
    // If not, ECHILD is the correct response.
    if !has_relevant_children() { return -(vfs::ECHILD as i64); }

    // Block and retry in a loop.
    // Spurious wakeups (timer ticks, other signals) must not cause ECHILD.
    loop {
        // Sleep until woken by exit_current → SIGCHLD → wake_process
        crate::sched::block_current(process::ProcessState::Sleeping);

        // Re-check for zombie after each wakeup
        if let Some(cpid) = find_zombie() {
            let code = process::reap_child(cpid).unwrap_or(0);
            if wstatus != 0 {
                unsafe { *(wstatus as *mut i32) = (code & 0xFF) << 8; }
            }
            return cpid as i64;
        }

        // Re-check whether we still have children (they may have all been reaped
        // by a concurrent wait in another thread, or all exited between wakeup
        // and this check).
        if !has_relevant_children() { return -(vfs::ECHILD as i64); }

        // Otherwise, a spurious wakeup — loop back and block again.
    }
}

pub fn sys_waitpid(pid: i32, wstatus: u64, options: i32) -> i64 {
    sys_wait4(pid, wstatus, options, 0)
}

pub fn sys_kill(pid: i32, sig: i32) -> i64 {
    // sig=0 is a validity check — just confirm target exists
    if sig < 0 || sig > 64 { return EINVAL; }
    let sig = sig as u32;

    if pid > 0 {
        // ESRCH if target doesn't exist
        if !process::all_pids().contains(&(pid as u32)) { return ESRCH; }
        if sig != 0 { signal::send_signal(pid as u32, sig); }
    } else if pid == 0 {
        let pgid = process::with_current(|p| p.pgid).unwrap_or(0);
        if sig != 0 { signal::send_signal_group(pgid, sig); }
    } else if pid == -1 {
        // Broadcast to all except init (pid 1) and self
        let me = process::current_pid();
        for p in process::all_pids() {
            if p != 1 && p != me && sig != 0 { signal::send_signal(p, sig); }
        }
    } else {
        if sig != 0 { signal::send_signal_group((-pid) as u32, sig); }
    }
    0
}

pub fn sys_tgkill(_tgid: i32, tid: i32, sig: i32) -> i64 { sys_kill(tid, sig) }
pub fn sys_tkill(tid: i32, sig: i32) -> i64 { sys_kill(tid, sig) }

pub fn sys_rt_sigaction(sig: i32, act: u64, oldact: u64, sigsetsize: usize) -> i64 {
    if sig <= 0 || sig >= signal::NSIG as i32 { return EINVAL; }
    let sig = sig as usize;

    // Return old action
    if oldact != 0 {
        let old = process::with_current(|p| p.sig_actions[sig]);
        #[repr(C)] struct KSigaction { handler: u64, flags: u64, restorer: u64, mask: u64 }
        let dst = unsafe { &mut *(oldact as *mut KSigaction) };
        if let Some(old_sa) = old {
        dst.handler  = match old_sa.handler { signal::SigHandler::User(h)=>h, signal::SigHandler::Ignore=>1, _=>0 };
        dst.flags    = old_sa.flags as u64;
        dst.restorer = old_sa.restorer;
        dst.mask     = old_sa.mask.0;
        }
    }

    // Set new action
    if act != 0 {
        #[repr(C)] struct KSigaction { handler: u64, flags: u64, restorer: u64, mask: u64 }
        let src = unsafe { &*(act as *const KSigaction) };
        let new_action = signal::SigAction {
            handler:  match src.handler { 0=>signal::SigHandler::Default, 1=>signal::SigHandler::Ignore, h=>signal::SigHandler::User(h) },
            flags:    src.flags as u32,
            restorer: src.restorer,
            mask:     signal::SignalSet(src.mask),
        };
        process::with_current_mut(|p| p.sig_actions[sig] = new_action);
    }
    0
}

pub fn sys_rt_sigprocmask(how: i32, set: u64, oldset: u64, sigsetsize: usize) -> i64 {
    const SIG_BLOCK: i32 = 0; const SIG_UNBLOCK: i32 = 1; const SIG_SETMASK: i32 = 2;
    if oldset != 0 {
        let m = process::with_current(|p| p.sig_mask.0).unwrap_or(0);
        unsafe { *(oldset as *mut u64) = m; }
    }
    if set != 0 {
        let new_mask = signal::SignalSet(unsafe { *(set as *const u64) });
        process::with_current_mut(|p| match how {
            SIG_BLOCK   => { for s in 0..64u32 { if new_mask.has(s) { p.sig_mask.add(s); } } }
            SIG_UNBLOCK => { for s in 0..64u32 { if new_mask.has(s) { p.sig_mask.remove(s); } } }
            SIG_SETMASK => { p.sig_mask = new_mask; }
            _            => {}
        });
    }
    0
}

pub fn sys_rt_sigreturn(frame: &mut crate::arch::x86_64::syscall_entry::SyscallFrame) -> i64 {
    signal::sigreturn(frame)
}

pub fn sys_sigaltstack(_ss: u64, _oss: u64) -> i64 { 0 }

pub fn sys_rt_sigsuspend(mask: u64, _sigsetsize: usize) -> i64 {
    if mask != 0 {
        let m = signal::SignalSet(unsafe { *(mask as *const u64) });
        process::with_current_mut(|p| p.sig_mask = m);
    }
    crate::sched::block_current(process::ProcessState::Sleeping);
    -4 // EINTR
}

// ── Scheduling ────────────────────────────────────────────────────────────

pub fn sys_sched_yield() -> i64 { crate::sched::schedule(); 0 }

pub fn sys_sched_setscheduler(pid: i32, policy: i32, param: u64) -> i64 {
    let target = if pid == 0 { process::current_pid() } else { pid as u32 };
    let prio   = if param != 0 { unsafe { *(param as *const u32) as u8 } } else { 0 };
    let qprio  = if policy == 1 || policy == 2 { prio.min(99) } else { crate::sched::PRIO_NORMAL };
    crate::sched::set_priority(target, qprio);
    0
}

pub fn sys_sched_getscheduler(pid: i32) -> i64 { crate::sched::SCHED_NORMAL as i64 }
pub fn sys_sched_setparam(_pid: i32, _param: u64) -> i64 { 0 }
pub fn sys_sched_getparam(_pid: i32, param: u64) -> i64 {
    if param != 0 { unsafe { *(param as *mut u32) = 0; } } 0
}
pub fn sys_sched_get_priority_max(_policy: i32) -> i64 { 99 }
pub fn sys_sched_get_priority_min(_policy: i32) -> i64 { 0 }
pub fn sys_nice(inc: i32) -> i64 {
    process::with_current_mut(|p| {
        p.nice = (p.nice as i32 + inc).clamp(-20, 19) as i8;
    });
    process::with_current(|p| p.nice as i64).unwrap_or(0)
}
pub fn sys_getpriority(which: i32, who: i32) -> i64 { 0 }
pub fn sys_setpriority(which: i32, who: i32, prio: i32) -> i64 {
    let nice = prio.clamp(-20, 19) as i8;
    process::with_current_mut(|p| p.nice = nice); 0
}

// ── Time ──────────────────────────────────────────────────────────────────

pub fn sys_nanosleep(req: u64, rem: u64) -> i64 {
    if req == 0 { return EFAULT; }
    let ts  = unsafe { &*(req as *const Timespec) };
    let ms  = ts.to_ms();
    crate::time::sleep_ms(ms);
    if rem != 0 { unsafe { *(rem as *mut Timespec) = Timespec { tv_sec: 0, tv_nsec: 0 }; } }
    0
}

pub fn sys_clock_nanosleep(_clk: i32, _flags: i32, req: u64, rem: u64) -> i64 {
    sys_nanosleep(req, rem)
}

pub fn sys_clock_gettime(clk: i32, tp: u64) -> i64 {
    if tp == 0 { return EFAULT; }
    let t = clock_gettime(clk);
    unsafe { *(tp as *mut Timespec) = t; }
    0
}

pub fn sys_clock_getres(clk: i32, res: u64) -> i64 {
    if res != 0 { unsafe { *(res as *mut Timespec) = Timespec { tv_sec: 0, tv_nsec: 1_000_000 }; } }
    0
}

pub fn sys_clock_settime(clk: i32, tp: u64) -> i64 {
    if tp == 0 { return EFAULT; }
    if clk != 0 { return EINVAL; } // Only CLOCK_REALTIME
    let ts = unsafe { &*(tp as *const Timespec) };
    crate::time::set_realtime(ts.tv_sec);
    0
}

pub fn sys_gettimeofday(tv: u64, tz: u64) -> i64 {
    if tv != 0 {
        let (s, us) = { let (s, ns) = crate::time::realtime(); (s, ns / 1000) };
        unsafe { *(tv as *mut i64) = s; *((tv+8) as *mut i64) = us; }
    }
    0
}

pub fn sys_settimeofday(tv: u64, tz: u64) -> i64 {
    if tv != 0 { let s = unsafe { *(tv as *const i64) }; crate::time::set_realtime(s); }
    0
}

pub fn sys_times(buf: u64) -> i64 {
    let t = crate::time::ticks() as i64;
    if buf != 0 { unsafe { core::ptr::write_bytes(buf as *mut u8, 0, 32); *(buf as *mut i64) = t; } }
    t
}

pub fn sys_alarm(secs: u32) -> i64 {
    // TODO: implement SIGALRM timer
    0
}

pub fn sys_setitimer(_which: i32, _new: u64, _old: u64) -> i64 { 0 }
pub fn sys_getitimer(_which: i32, _val: u64) -> i64 { 0 }
pub fn sys_timer_create(_clk: i32, _evp: u64, _timerid: u64) -> i64 { 0 }
pub fn sys_timer_settime(_id: i32, _f: i32, _new: u64, _old: u64) -> i64 { 0 }
pub fn sys_timer_gettime(_id: i32, _val: u64) -> i64 { 0 }
pub fn sys_timer_delete(_id: i32) -> i64 { 0 }

// ── System info ───────────────────────────────────────────────────────────

pub fn sys_uname(buf: u64) -> i64 {
    if buf == 0 { return EFAULT; }
    #[repr(C)] struct Uts { s:[u8;65], n:[u8;65], r:[u8;65], v:[u8;65], m:[u8;65], d:[u8;65] }
    let u = unsafe { &mut *(buf as *mut Uts) };
    fn fill(dst: &mut [u8], src: &[u8]) { let n=src.len().min(dst.len()-1); dst[..n].copy_from_slice(&src[..n]); dst[n]=0; }
    fill(&mut u.s, b"Linux");
    fill(&mut u.n, b"qunix");
    fill(&mut u.r, b"6.1.0-qunix");
    fill(&mut u.v, b"#1 SMP PREEMPT_DYNAMIC Qunix 0.2.0");
    fill(&mut u.m, b"x86_64");
    fill(&mut u.d, b"(none)");
    0
}

pub fn sys_sysinfo(buf: u64) -> i64 {
    if buf == 0 { return EFAULT; }
    #[repr(C)] struct SysInfo { uptime:i64, loads:[u64;3], totalram:u64, freeram:u64, sharedram:u64, bufferram:u64, totalswap:u64, freeswap:u64, procs:u16, _pad:[u8;22] }
    let s = unsafe { &mut *(buf as *mut SysInfo) };
    s.uptime   = (crate::time::ticks() / 1000) as i64;
    s.totalram = crate::memory::phys::total_frames() as u64 * 4096;
    s.freeram  = crate::memory::phys::free_frames() as u64 * 4096;
    s.procs    = process::all_pids().len() as u16;
    s.loads    = [0, 0, 0];
    0
}

pub fn sys_getrlimit(resource: i32, rlim: u64) -> i64 {
    if rlim == 0 { return EFAULT; }
    #[repr(C)] struct Rlimit { cur: u64, max: u64 }
    let r = unsafe { &mut *(rlim as *mut Rlimit) };
    // Default unlimited for most resources
    r.cur = match resource {
        3  => 8 * 1024 * 1024,        // RLIMIT_STACK = 8MB
        7  => 1024,                     // RLIMIT_NOFILE = 1024
        _  => u64::MAX,
    };
    r.max = u64::MAX;
    0
}

pub fn sys_setrlimit(_resource: i32, _rlim: u64) -> i64 { 0 }
pub fn sys_prlimit64(pid: i32, resource: i32, new_limit: u64, old_limit: u64) -> i64 {
    if old_limit != 0 { sys_getrlimit(resource, old_limit); }
    0
}

pub fn sys_getrusage(_who: i32, usage: u64) -> i64 {
    if usage != 0 { unsafe { core::ptr::write_bytes(usage as *mut u8, 0, 144); } } 0
}

// ── Misc ──────────────────────────────────────────────────────────────────

pub fn sys_ioctl(fd: i32, req: u64, arg: u64) -> i64 {
    // Route by fd kind first
    let kind = process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| f.kind.clone())
    }).flatten();

    if let Some(crate::vfs::FdKind::Drm) = &kind {
        return crate::drm::drm_ioctl(fd, req, arg);
    }
    if let Some(crate::vfs::FdKind::SeccompNotif(fd_id)) = kind {
        return crate::security::seccomp::notif_fd_ioctl(fd_id, req, arg);
    }

    // ── TTY ioctls ────────────────────────────────────────────────────
    // Any fd backed by a TTY device (minor 2, 3, 4) gets TTY handling.
    if crate::tty::is_tty_fd(fd) {
        return crate::tty::tty_ioctl(fd, req, arg);
    }

    // TCGETS on a non-TTY fd → ENOTTY (so isatty() works correctly)
    if req == crate::tty::TCGETS { return -25; } // ENOTTY

    // Plugin control ioctls — detected by ioctl number range 0x5100–0x5102
    if req >= crate::device::IOCTL_PLUGIN_ENABLE && req <= crate::device::IOCTL_PLUGIN_LIST {
        return crate::device::pluginctl_ioctl(req, arg);
    }

    crate::abi_compat::drm::handle_ioctl(fd, req, arg)
}

pub fn sys_prctl(op: i32, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    const PR_SET_NAME:     i32 = 15;
    const PR_GET_NAME:     i32 = 16;
    const PR_SET_DUMPABLE: i32 = 4;
    const PR_GET_DUMPABLE: i32 = 3;
    use crate::security::seccomp::{PR_SET_SECCOMP, PR_GET_SECCOMP,
        SECCOMP_MODE_STRICT, SECCOMP_MODE_FILTER};
    match op {
        PR_SET_NAME => {
            if a2 != 0 { let s = unsafe { read_cstr(a2) }.unwrap_or_default();
                process::with_current_mut(|p| p.name = s); }
            0
        }
        PR_GET_NAME => {
            if a2 != 0 { let name = process::with_current(|p| p.name.clone()).unwrap_or_default();
                let b = name.as_bytes(); let n = b.len().min(15);
                unsafe { core::ptr::copy_nonoverlapping(b.as_ptr(), a2 as *mut u8, n);
                    *((a2 + n as u64) as *mut u8) = 0; } }
            0
        }
        PR_SET_DUMPABLE | PR_GET_DUMPABLE => 1,
        PR_SET_SECCOMP => {
            crate::security::seccomp::sys_prctl_seccomp(a2, a3)
        }
        PR_GET_SECCOMP => {
            let mode = process::with_current(|p| p.seccomp.mode).unwrap_or(0);
            mode as i64
        }
        // PR_SET_NO_NEW_PRIVS (38) — prevents privilege escalation on exec
        38 => {
            process::with_current_mut(|p| { p.flags |= 0x0200_0000; });
            0
        }
        // PR_GET_NO_NEW_PRIVS (39)
        39 => {
            process::with_current(|p| if p.flags & 0x0200_0000 != 0 { 1i64 } else { 0i64 })
                .unwrap_or(0)
        }
        // PR_SET_CHILD_SUBREAPER (36)
        36 => 0,
        // PR_CAP_AMBIENT (47)
        47 => 0,
        _ => 0,
    }
}

pub fn sys_arch_prctl(code: i32, addr: u64) -> i64 {
    const ARCH_SET_FS: i32 = 0x1002;
    const ARCH_GET_FS: i32 = 0x1003;
    const ARCH_SET_GS: i32 = 0x1001;
    const ARCH_GET_GS: i32 = 0x1004;
    use crate::arch::x86_64::msr;
    match code {
        ARCH_SET_FS => { unsafe { msr::write(msr::IA32_FSBASE, addr); } 0 }
        ARCH_GET_FS => { let v = unsafe { msr::read(msr::IA32_FSBASE) }; if addr != 0 { unsafe { *(addr as *mut u64) = v; } } 0 }
        ARCH_SET_GS => { unsafe { msr::write(msr::IA32_GSBASE, addr); } 0 }
        ARCH_GET_GS => { let v = unsafe { msr::read(msr::IA32_GSBASE) }; if addr != 0 { unsafe { *(addr as *mut u64) = v; } } 0 }
        _ => EINVAL,
    }
}

pub fn sys_futex(addr: u64, op: i32, val: u32, timeout: u64, uaddr2: u64, val3: u32) -> i64 {
    crate::abi_compat::syscall::sys_futex(addr, op, val, timeout, uaddr2, val3)
}

pub fn sys_set_tid_address(_tidptr: u64) -> i64 { process::current_pid() as i64 }
pub fn sys_set_robust_list(_head: u64, _len: usize) -> i64 { 0 }
pub fn sys_get_robust_list(_pid: i32, _head: u64, _len: u64) -> i64 { 0 }

pub fn sys_getrandom(buf: u64, len: usize, _flags: u32) -> i64 {
    let tsc = unsafe { crate::arch::x86_64::msr::read(crate::arch::x86_64::msr::IA32_TSC) };
    let mut s = tsc ^ 0xDEAD_BEEF_CAFE_F00D;
    for i in 0..len {
        s ^= s << 13; s ^= s >> 7; s ^= s << 17;
        unsafe { *(buf as *mut u8).add(i) = s as u8; }
    }
    len as i64
}

pub fn sys_memfd_create(name: u64, flags: u32) -> i64 {
    // Create an anonymous file backed by tmpfs
    let nm = unsafe { read_cstr(name) }.unwrap_or_else(|| String::from("anon"));
    let path = alloc::format!("/tmp/.memfd_{}", nm);
    let cwd  = String::from("/");
    match vfs::open(&cwd, &path, O_RDWR | O_CREAT, 0o600) {
        Ok(fd) => process::with_current_mut(|p| p.alloc_fd(fd) as i64).unwrap_or(EBADF),
        Err(e) => vfs_err(e),
    }
}

pub fn sys_mknod(path: u64, mode: u32, dev: u64) -> i64 { 0 }
pub fn sys_mknodat(_dirfd: i32, path: u64, mode: u32, dev: u64) -> i64 { sys_mknod(path, mode, dev) }
pub fn sys_mount(_src: u64, _tgt: u64, _fstype: u64, _flags: u64, _data: u64) -> i64 { 0 }
pub fn sys_umount2(_tgt: u64, _flags: i32) -> i64 { 0 }

pub fn sys_select(nfds: i32, rfds: u64, wfds: u64, efds: u64, tv: u64) -> i64 {
    // Convert fd_set bitmaps to pollfd array and call sys_poll.
    // fd_set is a 128-byte bitmap (1024 bits) on Linux x86-64.
    if nfds <= 0 || nfds > 1024 { return 0; }

    let mut pollfds: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
    let mut fd_map: alloc::vec::Vec<(i32, i16)> = alloc::vec::Vec::new(); // (fd, events)

    for fd in 0..nfds {
        let byte  = (fd / 8) as usize;
        let bit   = (fd % 8) as u8;
        let mut events = 0i16;
        if rfds != 0 {
            let w = unsafe { *((rfds + byte as u64) as *const u8) };
            if w & (1 << bit) != 0 { events |= 0x0001; } // POLLIN
        }
        if wfds != 0 {
            let w = unsafe { *((wfds + byte as u64) as *const u8) };
            if w & (1 << bit) != 0 { events |= 0x0004; } // POLLOUT
        }
        if events != 0 {
            fd_map.push((fd, events));
        }
    }

    if fd_map.is_empty() { return 0; }

    // Build pollfd array on the kernel stack (pass as raw bytes to sys_poll)
    let mut pfd_buf: alloc::vec::Vec<u8> = alloc::vec![0u8; fd_map.len() * 8];
    for (i, &(fd, events)) in fd_map.iter().enumerate() {
        let base = i * 8;
        pfd_buf[base..base+4].copy_from_slice(&fd.to_le_bytes());
        pfd_buf[base+4..base+6].copy_from_slice(&events.to_le_bytes());
    }

    let tmo_ms = if tv == 0 {
        -1i32
    } else {
        let sec  = unsafe { *(tv       as *const i64) };
        let usec = unsafe { *((tv + 8) as *const i64) };
        (sec * 1000 + usec / 1000) as i32
    };

    let result = sys_poll(pfd_buf.as_ptr() as u64, fd_map.len() as u32, tmo_ms);
    if result <= 0 { return result; }

    // Clear and repopulate the fd_set bitmaps based on revents
    if rfds != 0 { unsafe { core::ptr::write_bytes(rfds as *mut u8, 0, 128); } }
    if wfds != 0 { unsafe { core::ptr::write_bytes(wfds as *mut u8, 0, 128); } }

    let mut total = 0i64;
    for (i, &(fd, _events)) in fd_map.iter().enumerate() {
        let revents = i16::from_le_bytes(pfd_buf[i*8+6..i*8+8].try_into().unwrap_or([0;2]));
        let byte = (fd / 8) as usize;
        let bit  = (fd % 8) as u8;
        if revents & 0x0001 != 0 && rfds != 0 { // POLLIN
            unsafe { *((rfds + byte as u64) as *mut u8) |= 1 << bit; }
            total += 1;
        }
        if revents & 0x0004 != 0 && wfds != 0 { // POLLOUT
            unsafe { *((wfds + byte as u64) as *mut u8) |= 1 << bit; }
            total += 1;
        }
    }
    total
}
pub fn sys_pselect6(n: i32, rfds: u64, wfds: u64, efds: u64, tv: u64, sig: u64) -> i64 { 0 }
pub fn sys_poll(fds: u64, n: u32, tmo: i32) -> i64 {
    if n == 0 { return 0; }
    if fds == 0 || fds >= 0x0000_8000_0000_0000 { return EFAULT; }

    // pollfd struct: { fd: i32, events: i16, revents: i16 }
    let POLLIN:  i16 = 0x0001;
    let POLLOUT: i16 = 0x0004;
    let POLLERR: i16 = 0x0008;
    let POLLHUP: i16 = 0x0010;
    let POLLNVAL:i16 = 0x0020;

    let check_ready = || -> i64 {
        let mut ready = 0i64;
        for i in 0..n as usize {
            let pfd_addr = fds + (i * 8) as u64;
            let pfd_fd     = unsafe { *(pfd_addr       as *const i32) };
            let pfd_events = unsafe { *((pfd_addr + 4) as *const i16) };
            let revents_ptr = (pfd_addr + 6) as *mut i16;

            // Zero out revents
            unsafe { *revents_ptr = 0; }

            if pfd_fd < 0 { continue; }

            let kind = process::with_current(|p| {
                p.get_fd(pfd_fd as u32).map(|f| f.kind.clone())
            }).flatten();

            match kind {
                None => {
                    unsafe { *revents_ptr = POLLNVAL; }
                    ready += 1;
                }
                Some(crate::vfs::FdKind::PipeRead(pipe)) => {
                    let (data_avail, write_closed) = {
                        let g = pipe.lock();
                        (g.len > 0, g.write_closed)
                    };
                    let mut rev = 0i16;
                    if pfd_events & POLLIN != 0 && (data_avail || write_closed) {
                        rev |= POLLIN;
                    }
                    if write_closed && !data_avail { rev |= POLLHUP; }
                    if rev != 0 { unsafe { *revents_ptr = rev; } ready += 1; }
                }
                Some(crate::vfs::FdKind::PipeWrite(pipe)) => {
                    let (space_avail, read_closed) = {
                        let g = pipe.lock();
                        (g.len < crate::ipc::pipe::PIPE_CAPACITY, g.read_closed)
                    };
                    let mut rev = 0i16;
                    if pfd_events & POLLOUT != 0 && space_avail && !read_closed {
                        rev |= POLLOUT;
                    }
                    if read_closed { rev |= POLLERR | POLLHUP; }
                    if rev != 0 { unsafe { *revents_ptr = rev; } ready += 1; }
                }
                Some(crate::vfs::FdKind::Device(2..=4)) => {
                    // TTY — readable if data in line buffer
                    let mut rev = 0i16;
                    if pfd_events & POLLIN  != 0 && crate::tty::tty_poll_readable() {
                        rev |= POLLIN;
                    }
                    if pfd_events & POLLOUT != 0 { rev |= POLLOUT; } // always writable
                    if rev != 0 { unsafe { *revents_ptr = rev; } ready += 1; }
                }
                Some(_) => {
                    // Regular files, devices: always ready
                    let mut rev = 0i16;
                    if pfd_events & POLLIN  != 0 { rev |= POLLIN;  }
                    if pfd_events & POLLOUT != 0 { rev |= POLLOUT; }
                    unsafe { *revents_ptr = rev; }
                    if rev != 0 { ready += 1; }
                }
            }
        }
        ready
    };

    // Non-blocking case or immediate readiness
    let ready = check_ready();
    if ready > 0 || tmo == 0 { return ready; }

    // Blocking poll: we must register as a waiter on every pipe fd so that
    // pipe_read/write wakes us when readiness changes. Without this, a pipe
    // becoming readable while we sleep would not wake us.
    let pid      = process::current_pid();
    let deadline = if tmo > 0 { Some(crate::time::ticks() + tmo as u64) } else { None };

    loop {
        // Collect all pipe Arcs we need to register with
        let mut read_pipes:  alloc::vec::Vec<alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuf>>> = alloc::vec::Vec::new();
        let mut write_pipes: alloc::vec::Vec<alloc::sync::Arc<spin::Mutex<crate::ipc::pipe::PipeBuf>>> = alloc::vec::Vec::new();
        let mut tty_poll = false;

        for i in 0..n as usize {
            let pfd_addr   = fds + (i * 8) as u64;
            let pfd_fd     = unsafe { *(pfd_addr as *const i32) };
            let pfd_events = unsafe { *((pfd_addr + 4) as *const i16) };
            if pfd_fd < 0 { continue; }

            let kind = process::with_current(|p| {
                p.get_fd(pfd_fd as u32).map(|f| f.kind.clone())
            }).flatten();

            match kind {
                Some(crate::vfs::FdKind::PipeRead(pipe)) => {
                    if pfd_events & 0x0001 != 0 { // POLLIN
                        crate::ipc::pipe::register_reader_waiter_pub(&pipe, pid);
                        read_pipes.push(pipe);
                    }
                }
                Some(crate::vfs::FdKind::PipeWrite(pipe)) => {
                    if pfd_events & 0x0004 != 0 { // POLLOUT
                        crate::ipc::pipe::register_writer_waiter_pub(&pipe, pid);
                        write_pipes.push(pipe);
                    }
                }
                Some(crate::vfs::FdKind::Device(2..=4)) => {
                    tty_poll = true;
                }
                _ => {}
            }
        }

        // Register with TTY reader queue if needed
        if tty_poll {
            crate::tty::register_poll_waiter(pid);
        }

        // Set deadline for sleep_ms-style timeout
        if let Some(dl) = deadline {
            let now = crate::time::ticks();
            if now >= dl {
                // Already past deadline — just check and return
                return check_ready();
            }
            crate::process::with_process_mut(pid, |p| {
                p.sleep_until = dl;
            });
        }

        // Block until something wakes us
        crate::sched::block_current(crate::process::ProcessState::Sleeping);

        // Clear sleep_until
        crate::process::with_process_mut(pid, |p| { p.sleep_until = 0; });

        // Check readiness after wakeup
        let r = check_ready();
        if r > 0 { return r; }

        // Check timeout
        if let Some(dl) = deadline {
            if crate::time::ticks() >= dl {
                return check_ready(); // Final check; return 0 if nothing ready
            }
        }

        // Spurious wakeup or not yet ready — loop and re-register
    }
}
pub fn sys_ppoll(fds: u64, n: u32, tmo: u64, _sig: u64, _sz: usize) -> i64 {
    // ppoll differs from poll in:
    // 1. Timeout is a struct timespec (sec + nsec) not milliseconds
    // 2. Atomically installs a signal mask while waiting (we skip this for now)
    // Convert timespec to milliseconds and delegate to sys_poll.
    let tmo_ms: i32 = if tmo == 0 {
        -1 // NULL timeout = infinite
    } else if tmo >= 0x0000_8000_0000_0000 {
        return EFAULT;
    } else {
        let ts = unsafe { &*(tmo as *const [i64; 2]) };
        let ms = (ts[0].max(0) as u64) * 1000 + (ts[1].max(0) as u64) / 1_000_000;
        ms.min(i32::MAX as u64) as i32
    };
    sys_poll(fds, n, tmo_ms)
}

pub fn sys_epoll_create1(flags: i32) -> i64 { crate::ipc::epoll::epoll_create(flags) }
pub fn sys_epoll_create(flags: i32)  -> i64 { crate::ipc::epoll::epoll_create(flags) }
pub fn sys_epoll_ctl(epfd: i32, op: i32, fd: i32, ev: u64) -> i64 { crate::ipc::epoll::epoll_ctl(epfd, op, fd, ev) }
pub fn sys_epoll_wait(epfd: i32, evs: u64, max: i32, tmo: i32) -> i64 { crate::ipc::epoll::epoll_wait(epfd, evs, max, tmo) }
pub fn sys_epoll_pwait(epfd: i32, evs: u64, max: i32, tmo: i32, sig: u64, sz: usize) -> i64 { crate::ipc::epoll::epoll_wait(epfd, evs, max, tmo) }

// Sockets
pub fn sys_socket(fam: i32, typ: i32, proto: i32) -> i64 { crate::net::socket::sys_socket(fam as u16, typ as u8, proto as u8) as i64 }
pub fn sys_bind(fd: i32, addr: u64, len: u32) -> i64 { crate::net::socket::sys_bind(fd, addr, len) as i64 }
pub fn sys_listen(fd: i32, bl: i32) -> i64 { crate::net::socket::sys_listen(fd, bl) as i64 }
pub fn sys_connect(fd: i32, addr: u64, len: u32) -> i64 { crate::net::socket::sys_connect(fd, addr, len) as i64 }
pub fn sys_accept(fd: i32, addr: u64, len: u64) -> i64 { crate::net::socket::sys_accept(fd, addr, len) as i64 }
pub fn sys_accept4(fd: i32, addr: u64, len: u64, flags: i32) -> i64 { crate::net::socket::sys_accept(fd, addr, len) as i64 }
pub fn sys_sendto(fd: i32, buf: u64, len: usize, flags: i32, addr: u64, alen: u32) -> i64 { crate::net::socket::sys_sendto(fd, buf, len, flags, addr, alen) }
pub fn sys_recvfrom(fd: i32, buf: u64, len: usize, flags: i32, addr: u64, alen: u64) -> i64 { crate::net::socket::sys_recvfrom(fd, buf, len, flags, addr, alen) }
pub fn sys_setsockopt(fd: i32, lvl: i32, opt: i32, val: u64, vlen: u32) -> i64 { crate::net::socket::sys_setsockopt(fd, lvl, opt, val, vlen) as i64 }
pub fn sys_getsockopt(fd: i32, lvl: i32, opt: i32, val: u64, vlen: u64) -> i64 { crate::net::socket::sys_getsockopt(fd, lvl, opt, val, vlen) as i64 }
pub fn sys_getsockname(fd: i32, addr: u64, len: u64) -> i64 { crate::net::socket::sys_getsockname(fd, addr, len) as i64 }
pub fn sys_getpeername(fd: i32, addr: u64, len: u64) -> i64 { crate::net::socket::sys_getpeername(fd, addr, len) as i64 }
pub fn sys_shutdown(fd: i32, how: i32) -> i64 { crate::net::socket::sys_shutdown(fd, how) as i64 }
pub fn sys_sendmsg(fd: i32, msg: u64, flags: i32) -> i64 { 0 }
pub fn sys_recvmsg(fd: i32, msg: u64, flags: i32) -> i64 { 0 }
pub fn sys_sendmmsg(fd: i32, mmsg: u64, vlen: u32, flags: i32) -> i64 { 0 }
pub fn sys_recvmmsg(fd: i32, mmsg: u64, vlen: u32, flags: i32, tmo: u64) -> i64 { 0 }
pub fn sys_socketpair(fam: i32, typ: i32, proto: i32, sv: u64) -> i64 {
    let (r, w) = crate::ipc::pipe::new_pipe();
    let (rfd, wfd) = process::with_current_mut(|p| (p.alloc_fd(r), p.alloc_fd(w))).unwrap_or((0, 0));
    unsafe { *(sv as *mut u32) = rfd; *((sv+4) as *mut u32) = wfd; }
    0
}

// Shared memory / semaphores
pub fn sys_shm_open(name: u64, flags: i32, mode: u32) -> i64 {
    let s = match unsafe { read_cstr(name) } { Some(s)=>s, None=>return EFAULT };
    crate::ipc::shm::shm_open(&s, flags, mode)
}
pub fn sys_shm_unlink(name: u64) -> i64 {
    let s = match unsafe { read_cstr(name) } { Some(s)=>s, None=>return EFAULT };
    crate::ipc::shm::shm_unlink(&s)
}
pub fn sys_sem_open(name: u64, flag: i32, mode: u32, val: u32) -> i64 {
    let s = match unsafe { read_cstr(name) } { Some(s)=>s, None=>return EFAULT };
    crate::ipc::sem::sem_open(&s, flag, mode, val) as i64
}
pub fn sys_sem_wait(name: u64) -> i64 {
    let s = match unsafe { read_cstr(name) } { Some(s)=>s, None=>return EFAULT };
    crate::ipc::sem::sem_wait(&s) as i64
}
pub fn sys_sem_post(name: u64) -> i64 {
    let s = match unsafe { read_cstr(name) } { Some(s)=>s, None=>return EFAULT };
    crate::ipc::sem::sem_post(&s) as i64
}

// Linux-compat misc
pub fn sys_sendfile(out_fd: i32, in_fd: i32, offset: u64, count: usize) -> i64 {
    // Naive: read from in_fd and write to out_fd
    let mut total = 0i64;
    let mut buf = alloc::vec![0u8; 65536];
    let mut remaining = count;
    while remaining > 0 {
        let chunk = remaining.min(65536);
        let n = sys_read(in_fd, buf.as_mut_ptr() as u64, chunk);
        if n <= 0 { break; }
        let w = sys_write(out_fd, buf.as_ptr() as u64, n as usize);
        if w < 0 { break; }
        total += w;
        remaining -= w as usize;
    }
    total
}

pub fn sys_copy_file_range(in_fd: i32, _in_off: u64, out_fd: i32, _out_off: u64, len: usize, _flags: u32) -> i64 {
    sys_sendfile(out_fd, in_fd, 0, len)
}

pub fn sys_inotify_init1(_flags: i32) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_inotify_add_watch(_fd: i32, _path: u64, _mask: u32) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_inotify_rm_watch(_fd: i32, _wd: i32) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_io_uring_setup(_entries: u32, _params: u64) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_io_uring_enter(_fd: i32, _to: u32, _min: u32, _flags: u32, _sig: u64, _sz: usize) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_io_uring_register(_fd: i32, _op: u32, _arg: u64, _nr: u32) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_pidfd_open(_pid: i32, _flags: u32) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_landlock_create_ruleset(_attr: u64, _sz: usize, _flags: u32) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_seccomp(op: u32, flags: u32, args: u64) -> i64 {
    crate::security::seccomp::sys_seccomp_real(op, flags, args)
}
pub fn sys_ptrace(_req: i32, _pid: i32, _addr: u64, _data: u64) -> i64 { EPERM }
pub fn sys_process_vm_readv(_pid: i32, _lv: u64, _lc: u64, _rv: u64, _rc: u64, _f: u64) -> i64 { EPERM }
pub fn sys_process_vm_writev(_pid: i32, _lv: u64, _lc: u64, _rv: u64, _rc: u64, _f: u64) -> i64 { EPERM }
pub fn sys_statx(dirfd: i32, path: u64, flags: i32, mask: u32, buf: u64) -> i64 {
    sys_newfstatat(dirfd, path, buf, flags)
}


// ── Additional handlers needed by the full dispatch table ─────────────────

pub fn sys_flock(_fd: i32, _op: i32) -> i64 { 0 }
pub fn sys_chroot(path: u64) -> i64 { 0 } // TODO: implement chroot jail
pub fn sys_sethostname(name: u64, len: usize) -> i64 { 0 }
pub fn sys_setdomainname(name: u64, len: usize) -> i64 { 0 }
pub fn sys_reboot(magic1: i32, magic2: i32, cmd: i32, arg: u64) -> i64 {
    if magic1 != -0x28121969 { return EINVAL; }
    match cmd {
        0x01234567 => { crate::klog!("REBOOT requested"); 0 }
        0x4321FEDC => { crate::klog!("HALT requested"); unsafe { crate::arch::x86_64::cpu::halt_forever() }; 0 }
        _ => 0,
    }
}
pub fn sys_init_module(buf: u64, len: usize, args: u64) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_delete_module(name: u64, flags: u32) -> i64 { -(vfs::ENOSYS as i64) }
pub fn sys_gettid() -> i64 { process::current_pid() as i64 }
pub fn sys_readahead(_fd: i32, _off: i64, _count: usize) -> i64 { 0 }
pub fn sys_utime(_path: u64, _times: u64) -> i64 { 0 }
pub fn sys_utimes(_path: u64, _times: u64) -> i64 { 0 }
pub fn sys_utimensat(_dirfd: i32, _path: u64, _times: u64, _flags: i32) -> i64 { 0 }
pub fn sys_futimesat(_dirfd: i32, _path: u64, _times: u64) -> i64 { 0 }
pub fn sys_capget(_hdr: u64, _data: u64) -> i64 { 0 }
pub fn sys_capset(_hdr: u64, _data: u64) -> i64 { 0 }
pub fn sys_setreuid(ruid: u32, euid: u32) -> i64 {
    process::with_current_mut(|p| { if ruid != u32::MAX { p.uid = ruid; } if euid != u32::MAX { p.euid = euid; } });
    0
}
pub fn sys_setregid(rgid: u32, egid: u32) -> i64 {
    process::with_current_mut(|p| { if rgid != u32::MAX { p.gid = rgid; } if egid != u32::MAX { p.egid = egid; } });
    0
}
pub fn sys_getresuid(ruid: u64, euid: u64, suid: u64) -> i64 {
    let (r,e,s) = process::with_current(|p| (p.uid, p.euid, p.suid)).unwrap_or((0,0,0));
    if ruid != 0 { unsafe { *(ruid as *mut u32) = r; } }
    if euid != 0 { unsafe { *(euid as *mut u32) = e; } }
    if suid != 0 { unsafe { *(suid as *mut u32) = s; } }
    0
}
pub fn sys_getresgid(rgid: u64, egid: u64, sgid: u64) -> i64 {
    let (r,e,s) = process::with_current(|p| (p.gid, p.egid, p.sgid)).unwrap_or((0,0,0));
    if rgid != 0 { unsafe { *(rgid as *mut u32) = r; } }
    if egid != 0 { unsafe { *(egid as *mut u32) = e; } }
    if sgid != 0 { unsafe { *(sgid as *mut u32) = s; } }
    0
}
pub fn sys_setfsuid(fsuid: u32) -> i64 { process::with_current(|p| p.euid as i64).unwrap_or(0) }
pub fn sys_setfsgid(fsgid: u32) -> i64 { process::with_current(|p| p.egid as i64).unwrap_or(0) }
pub fn sys_rt_sigtimedwait(set: u64, info: u64, tmo: u64, sz: usize) -> i64 { -4 } // EINTR
pub fn sys_restart_syscall() -> i64 { -4 }
pub fn sys_fadvise64(_fd: i32, _off: i64, _len: usize, _advice: i32) -> i64 { 0 }
pub fn sys_timer_getoverrun(_id: i32) -> i64 { 0 }
pub fn sys_io_setup(_nr_events: u32, _ctxp: u64) -> i64 { -38 }
pub fn sys_io_destroy(_ctx_id: u32) -> i64 { -38 }
pub fn sys_io_getevents(_ctx: u32, _min: i64, _nr: i64, _events: u64, _tmo: u64) -> i64 { 0 }
pub fn sys_io_submit(_ctx: u32, _nr: i64, _iocbpp: u64) -> i64 { 0 }
pub fn sys_io_cancel(_ctx: u32, _iocb: u64, _result: u64) -> i64 { -38 }
pub fn sys_io_pgetevents(_ctx: u32, _min: i64, _nr: i64, _events: u64, _tmo: u64, _sig: u64) -> i64 { 0 }
pub fn sys_sched_setaffinity(_pid: i32, _size: usize, _mask: u64) -> i64 { 0 }
pub fn sys_sched_getaffinity(_pid: i32, size: usize, mask: u64) -> i64 {
    if mask != 0 && size >= 8 { unsafe { *(mask as *mut u64) = 1; } } 0
}
pub fn sys_mbind(_addr: u64, _len: u64, _mode: i32, _nmask: u64, _maxnode: u64, _flags: u32) -> i64 { 0 }
pub fn sys_set_mempolicy(_mode: i32, _nmask: u64, _maxnode: u64) -> i64 { 0 }
pub fn sys_get_mempolicy(_mode: u64, _nmask: u64, _maxnode: u64, _addr: u64, _flags: u64) -> i64 { 0 }
pub fn sys_waitid(_idtype: i32, _id: i32, _info: u64, _options: i32, _rusage: u64) -> i64 { -10 }
pub fn sys_ioprio_set(_which: i32, _who: i32, _ioprio: i32) -> i64 { 0 }
pub fn sys_ioprio_get(_which: i32, _who: i32) -> i64 { 0 }
pub fn sys_signalfd(_fd: i32, _mask: u64, _size: usize) -> i64 { -38 }
pub fn sys_signalfd4(_fd: i32, _mask: u64, _size: usize, _flags: i32) -> i64 { -38 }
pub fn sys_timerfd_create(_clkid: i32, _flags: i32) -> i64 { -38 }
pub fn sys_timerfd_settime(_fd: i32, _flags: i32, _new: u64, _old: u64) -> i64 { -38 }
pub fn sys_timerfd_gettime(_fd: i32, _val: u64) -> i64 { -38 }
pub fn sys_eventfd(initval: u32) -> i64 { -38 }
pub fn sys_eventfd2(initval: u32, flags: i32) -> i64 { -38 }
pub fn sys_fallocate(_fd: i32, _mode: i32, _off: i64, _len: i64) -> i64 { 0 }
pub fn sys_splice(_fd_in: i32, _off_in: u64, _fd_out: i32, _off_out: u64, _len: usize, _flags: u32) -> i64 { 0 }
pub fn sys_tee(_fd_in: i32, _fd_out: i32, _len: usize, _flags: u32) -> i64 { 0 }
pub fn sys_sync_file_range(_fd: i32, _off: i64, _nbytes: i64, _flags: u32) -> i64 { 0 }
pub fn sys_vmsplice(_fd: i32, _iov: u64, _nr: usize, _flags: u32) -> i64 { 0 }
pub fn sys_move_pages(_pid: i32, _nr: u64, _pages: u64, _nodes: u64, _status: u64, _flags: i32) -> i64 { 0 }
pub fn sys_getcpu(cpu: u64, node: u64, _cache: u64) -> i64 {
    if cpu  != 0 { unsafe { *(cpu  as *mut u32) = 0; } }
    if node != 0 { unsafe { *(node as *mut u32) = 0; } }
    0
}
pub fn sys_fanotify_init(_flags: u32, _event_f_flags: u32) -> i64 { -38 }
pub fn sys_fanotify_mark(_fd: i32, _flags: u32, _mask: u64, _dirfd: i32, _path: u64) -> i64 { -38 }
pub fn sys_name_to_handle_at(_df: i32, _path: u64, _handle: u64, _mnt_id: u64, _flags: i32) -> i64 { -38 }
pub fn sys_open_by_handle_at(_mountfd: i32, _handle: u64, _flags: i32) -> i64 { -38 }
pub fn sys_clock_adjtime(_clk: i32, _tx: u64) -> i64 { 0 }
pub fn sys_syncfs(_fd: i32) -> i64 { sys_sync() }
pub fn sys_setns(_fd: i32, _nstype: i32) -> i64 { 0 }
pub fn sys_linkat(_old_dirfd: i32, old: u64, _new_dirfd: i32, new: u64, _flags: i32) -> i64 { sys_link(old, new) }
pub fn sys_readlinkat(_dirfd: i32, path: u64, buf: u64, size: usize) -> i64 { sys_readlink(path, buf, size) }
pub fn sys_preadv(_fd: i32, _iov: u64, _iovcnt: usize, _off: i64) -> i64 { 0 }
pub fn sys_pwritev(_fd: i32, _iov: u64, _iovcnt: usize, _off: i64) -> i64 { 0 }
pub fn sys_preadv2(_fd: i32, _iov: u64, _iovcnt: usize, _off: i64, _flags: i32) -> i64 { 0 }
pub fn sys_pwritev2(_fd: i32, _iov: u64, _iovcnt: usize, _off: i64, _flags: i32) -> i64 { 0 }
pub fn sys_rseq(_rseq: u64, _rseq_len: u32, _flags: i32, _sig: u32) -> i64 { -38 }
pub fn sys_kexec_file_load(_kernel_fd: i32, _initrd_fd: i32, _nr: usize, _args: u64, _flags: u64) -> i64 { -38 }
pub fn sys_bpf(_cmd: i32, _attr: u64, _size: u32) -> i64 { -1 }
pub fn sys_execveat(_dirfd: i32, path: u64, argv: u64, envp: u64, _flags: i32) -> i64 { sys_execve(path, argv, envp) }
pub fn sys_userfaultfd(_flags: i32) -> i64 { -38 }
pub fn sys_membarrier(_cmd: i32, _flags: u32, _cpu: i32) -> i64 { 0 }
pub fn sys_mlock2(_addr: u64, _len: u64, _flags: u32) -> i64 { 0 }
pub fn sys_pkey_mprotect(_addr: u64, _len: u64, _prot: i32, _pkey: i32) -> i64 { 0 }
pub fn sys_pkey_alloc(_flags: u32, _init: u32) -> i64 { 0 }
pub fn sys_pkey_free(_pkey: i32) -> i64 { 0 }
pub fn sys_sched_setattr(_pid: i32, _attr: u64, _flags: u32) -> i64 { 0 }
pub fn sys_sched_getattr(_pid: i32, _attr: u64, _size: u32, _flags: u32) -> i64 { 0 }
pub fn sys_finit_module(_fd: i32, _args: u64, _flags: i32) -> i64 { -38 }
pub fn sys_kcmp(_pid1: i32, _pid2: i32, _type_: i32, _idx1: u64, _idx2: u64) -> i64 { 0 }
pub fn sys_pidfd_send_signal(_pidfd: i32, sig: i32, _info: u64, _flags: u32) -> i64 { 0 }
pub fn sys_clone3(_cl_args: u64, _size: usize) -> i64 { sys_fork() }
pub fn sys_close_range(lo: u32, hi: u32, _flags: u32) -> i64 {
    for fd in lo..=hi.min(1023) { sys_close(fd as i32); }
    0
}
pub fn sys_openat2(_dirfd: i32, path: u64, how: u64, _size: usize) -> i64 {
    let flags = if how != 0 { unsafe { *(how as *const i32) } } else { 0 };
    let mode  = if how != 0 { unsafe { *((how+8) as *const u32) } } else { 0o666 };
    sys_open(path, flags, mode)
}
pub fn sys_pidfd_getfd(_pidfd: i32, _targetfd: i32, _flags: u32) -> i64 { -38 }
pub fn sys_faccessat2(_dirfd: i32, path: u64, mode: i32, _flags: i32) -> i64 { sys_access(path, mode) }
pub fn sys_open_tree(_dirfd: i32, _path: u64, _flags: u32) -> i64 { -38 }
pub fn sys_move_mount(_from_dirfd: i32, _from_path: u64, _to_dirfd: i32, _to_path: u64, _flags: u32) -> i64 { -38 }
pub fn sys_fsopen(_fs_name: u64, _flags: u32) -> i64 { -38 }
pub fn sys_fsconfig(_fd: i32, _cmd: u32, _key: u64, _value: u64, _aux: i32) -> i64 { -38 }
pub fn sys_fsmount(_fd: i32, _flags: u32, _attr_flags: u32) -> i64 { -38 }
pub fn sys_fspick(_dirfd: i32, _path: u64, _flags: u32) -> i64 { -38 }

pub fn sys_mremap(old_addr: u64, old_len: usize, new_len: usize, flags: i32, new_addr: u64) -> i64 {
    if old_len == 0 && flags as u32 & crate::memory::vmm::MREMAP_MAYMOVE == 0 {
        return EINVAL;
    }
    if new_len == 0 { return EINVAL; }

    // Validate old_addr is page-aligned and in user space
    if old_addr & 0xFFF != 0 || old_addr >= 0x0000_8000_0000_0000 {
        return EINVAL;
    }

    match process::with_current_mut(|proc| {
        proc.address_space.mremap(
            old_addr,
            old_len as u64,
            new_len as u64,
            flags as u32,
            new_addr,
        )
    }).flatten() {
        Some(new_ptr) => new_ptr as i64,
        None          => ENOMEM,
    }
}
pub fn sys_mincore(addr: u64, _len: usize, _vec: u64) -> i64 { 0 }
pub fn sys_syslog(msg: u64) -> i64 { 0 }
pub fn sys_fcntl_dummy() -> i64 { 0 }
pub fn sys_creat(path: u64, mode: u32) -> i64 {
    sys_open(path, crate::vfs::O_WRONLY | crate::vfs::O_CREAT | crate::vfs::O_TRUNC, mode)
}
