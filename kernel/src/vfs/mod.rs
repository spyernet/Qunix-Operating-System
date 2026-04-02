use alloc::borrow::ToOwned;
// Virtual Filesystem Switch — unified interface over all filesystems.
//
// Concepts:
//   Superblock: represents a mounted filesystem instance.
//   Inode:      represents a file/directory/symlink.
//   Dentry:     path component cached for lookup speed.
//   FileDescriptor: open file descriptor with offset and flags.
//   MountPoint: binding of a Superblock to a VFS path.

use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use spin::Mutex;

// ── Error codes (POSIX errno values) ─────────────────────────────────────

pub type VfsError = u32;
pub const EPERM:    VfsError = 1;
pub const ENOENT:   VfsError = 2;
pub const EIO:      VfsError = 5;
pub const EBADF:    VfsError = 9;
pub const EAGAIN:   VfsError = 11;
pub const ENOMEM:   VfsError = 12;
pub const EACCES:   VfsError = 13;
pub const EFAULT:   VfsError = 14;
pub const EBUSY:    VfsError = 16;
pub const EEXIST:   VfsError = 17;
pub const EXDEV:    VfsError = 18;
pub const ENODEV:   VfsError = 19;
pub const ENOTDIR:  VfsError = 20;
pub const EISDIR:   VfsError = 21;
pub const EINVAL:   VfsError = 22;
pub const EMFILE:   VfsError = 24;
pub const ENOSPC:   VfsError = 28;
pub const EPIPE:    VfsError = 32;
pub const ERANGE:   VfsError = 34;
pub const ECHILD:   VfsError = 10;
pub const ENOSYS:   VfsError = 38;
pub const ENOTEMPTY: VfsError = 39;
pub const ELOOP:    VfsError = 40;
pub const ENAMETOOLONG: VfsError = 36;

// ── Mode bits ─────────────────────────────────────────────────────────────

pub const S_IFMT:   u32 = 0xF000;
pub const S_IFSOCK: u32 = 0xC000;
pub const S_IFLNK:  u32 = 0xA000;
pub const S_IFREG:  u32 = 0x8000;
pub const S_IFBLK:  u32 = 0x6000;
pub const S_IFDIR:  u32 = 0x4000;
pub const S_IFCHR:  u32 = 0x2000;
pub const S_IFIFO:  u32 = 0x1000;

pub fn s_isreg(m: u32) -> bool  { m & S_IFMT == S_IFREG }
pub fn s_isdir(m: u32) -> bool  { m & S_IFMT == S_IFDIR }
pub fn s_islnk(m: u32) -> bool  { m & S_IFMT == S_IFLNK }
pub fn s_ischr(m: u32) -> bool  { m & S_IFMT == S_IFCHR }
pub fn s_isblk(m: u32) -> bool  { m & S_IFMT == S_IFBLK }
pub fn s_isfifo(m: u32) -> bool { m & S_IFMT == S_IFIFO }

// ── Open flags ────────────────────────────────────────────────────────────

pub const O_RDONLY:    i32 = 0;
pub const O_WRONLY:    i32 = 1;
pub const O_RDWR:      i32 = 2;
pub const O_CREAT:     i32 = 0o100;
pub const O_EXCL:      i32 = 0o200;
pub const O_TRUNC:     i32 = 0o1000;
pub const O_APPEND:    i32 = 0o2000;
pub const O_NONBLOCK:  i32 = 0o4000;
pub const O_CLOEXEC:   i32 = 0o2000000;
pub const O_DIRECTORY: i32 = 0o200000;
pub const O_PATH:      i32 = 0o10000000;

// ── stat structure ────────────────────────────────────────────────────────

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Stat {
    pub st_dev:     u64,
    pub st_ino:     u64,
    pub st_nlink:   u64,
    pub st_mode:    u32,
    pub st_uid:     u32,
    pub st_gid:     u32,
    pub _pad0:      u32,
    pub st_rdev:    u64,
    pub st_size:    i64,
    pub st_blksize: i64,
    pub st_blocks:  i64,
    pub st_atime:   i64,
    pub st_atime_ns: i64,
    pub st_mtime:   i64,
    pub st_mtime_ns: i64,
    pub st_ctime:   i64,
    pub st_ctime_ns: i64,
    pub _unused:    [i64; 3],
}

// ── Inode ─────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct Inode {
    pub ino:   u64,
    pub mode:  u32,
    pub uid:   u32,
    pub gid:   u32,
    pub size:  u64,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub ops:   Arc<dyn InodeOps>,
    pub sb:    Arc<Superblock>,
}

impl Inode {
    pub fn to_stat(&self) -> Stat {
        Stat {
            st_dev:   self.sb.dev,
            st_ino:   self.ino,
            st_nlink: 1,
            st_mode:  self.mode,
            st_uid:   self.uid,
            st_gid:   self.gid,
            st_size:  self.size as i64,
            st_blksize: 4096,
            st_blocks: ((self.size + 511) / 512) as i64,
            st_atime:  self.atime,
            st_mtime:  self.mtime,
            st_ctime:  self.ctime,
            ..Default::default()
        }
    }
}

// ── InodeOps trait ────────────────────────────────────────────────────────

pub trait InodeOps: Send + Sync {
    fn read(&self,    inode: &Inode, buf: &mut [u8], offset: u64) -> Result<usize, VfsError>;
    fn write(&self,   inode: &Inode, buf: &[u8],     offset: u64) -> Result<usize, VfsError>;
    fn readdir(&self, inode: &Inode, offset: u64) -> Result<Vec<DirEntry>, VfsError>;
    fn lookup(&self,  inode: &Inode, name: &str) -> Result<Inode, VfsError>;

    fn create(&self, _: &Inode, _name: &str, _mode: u32) -> Result<Inode, VfsError>         { Err(EACCES) }
    fn mkdir(&self,  _: &Inode, _name: &str, _mode: u32) -> Result<Inode, VfsError>         { Err(EACCES) }
    fn rmdir(&self,  _: &Inode, _name: &str) -> Result<(), VfsError>                         { Err(EACCES) }
    fn unlink(&self, _: &Inode, _name: &str) -> Result<(), VfsError>                         { Err(EACCES) }
    fn symlink(&self,_: &Inode, _name: &str, _target: &str) -> Result<Inode, VfsError>      { Err(EACCES) }
    fn readlink(&self,_: &Inode) -> Result<String, VfsError>                                 { Err(EINVAL) }
    fn truncate(&self,_: &Inode, _size: u64) -> Result<(), VfsError>                        { Err(EACCES) }
    fn rename(&self, _: &Inode, _old: &str, _new_parent: u64, _new: &str) -> Result<(), VfsError> { Err(EACCES) }
    fn chmod(&self,  _: &Inode, _mode: u32) -> Result<(), VfsError>                         { Ok(()) }
    fn chown(&self,  _: &Inode, _uid: u32, _gid: u32) -> Result<(), VfsError>               { Ok(()) }
    fn link(&self,   _: &Inode, _name: &str, _target: &Inode) -> Result<(), VfsError>       { Err(EACCES) }
    fn fsync(&self,  _: &Inode) -> Result<(), VfsError>                                      { Ok(()) }
    fn mmap_get_phys(&self, _: &Inode, _offset: u64) -> Option<u64>                         { None }
    /// For character/block devices: return the device minor number.
    /// VFS uses this to set FdKind::Device(minor) on open.
    fn device_minor(&self) -> Option<u32>                                                    { None }
}

// ── DirEntry ──────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct DirEntry {
    pub name:      String,
    pub ino:       u64,
    pub file_type: u8, // 4=dir, 8=reg, 10=lnk, 2=chr, 6=blk, 1=fifo
}

// ── Superblock ────────────────────────────────────────────────────────────

pub struct Superblock {
    pub dev:     u64,
    pub fs_type: String,
    pub ops:     Arc<dyn SuperblockOps>,
}

pub trait SuperblockOps: Send + Sync {
    fn get_root(&self) -> Result<Inode, VfsError>;
    fn sync(&self) {}
    fn statfs(&self, _buf: &mut StatFs) {}
}

#[repr(C)]
#[derive(Default)]
pub struct StatFs {
    pub f_type:    i64,
    pub f_bsize:   i64,
    pub f_blocks:  u64,
    pub f_bfree:   u64,
    pub f_bavail:  u64,
    pub f_files:   u64,
    pub f_ffree:   u64,
    pub f_fsid:    [i32; 2],
    pub f_namelen: i64,
    pub f_frsize:  i64,
    pub f_flags:   i64,
    pub f_spare:   [i64; 4],
}

// ── File descriptor kinds ─────────────────────────────────────────────────

#[derive(Clone)]
pub enum FdKind {
    Regular,
    Directory,
    PipeRead(Arc<spin::Mutex<crate::ipc::pipe::PipeBuf>>),
    PipeWrite(Arc<spin::Mutex<crate::ipc::pipe::PipeBuf>>),
    Device(u32),
    Drm,
    Socket(u32),      // socket fd index
    Epoll(u32),       // epoll instance id
    IoUring(u32),     // io_uring instance fd
    SeccompNotif(u32),// seccomp notification fd
}

// ── File descriptor ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct FileDescriptor {
    pub inode:   Inode,
    pub offset:  u64,
    pub flags:   i32,
    pub kind:    FdKind,
    /// Absolute path this descriptor was opened at.
    /// Used by openat(dirfd, relpath) to resolve relative paths.
    pub path:    String,
}

// ── Mount table ───────────────────────────────────────────────────────────

pub struct MountPoint {
    pub path: String,
    pub sb:   Arc<Superblock>,
}

pub static MOUNTS: Mutex<Vec<MountPoint>> = Mutex::new(Vec::new());

pub fn init() {
    MOUNTS.lock().clear();
}

pub fn mount(path: &str, sb: Arc<Superblock>) {
    let mut m = MOUNTS.lock();
    // Remove existing mount at this path
    m.retain(|mp| mp.path != path);
    m.push(MountPoint { path: String::from(path), sb });
    crate::klog!("VFS: mounted at '{}'", path);
}

pub fn umount(path: &str) -> bool {
    let mut m = MOUNTS.lock();
    let before = m.len();
    m.retain(|mp| mp.path != path);
    m.len() < before
}

// ── Path resolution ───────────────────────────────────────────────────────

fn find_mount(abs: &str) -> Option<(Arc<Superblock>, String)> {
    let m = MOUNTS.lock();
    let mut best: Option<(usize, &MountPoint)> = None;
    for mp in m.iter() {
        if abs.starts_with(mp.path.as_str()) {
            let len = mp.path.len();
            // Ensure we match on a component boundary
            let rest = &abs[len..];
            if rest.is_empty() || rest.starts_with('/') || mp.path == "/" {
                if best.map_or(true, |(l, _)| len > l) {
                    best = Some((len, mp));
                }
            }
        }
    }
    best.map(|(len, mp)| {
        let rel = if len < abs.len() { abs[len..].trim_start_matches('/') } else { "" };
        let rel = if rel.is_empty() { "/" } else { rel };
        (mp.sb.clone(), String::from(rel))
    })
}

fn abs_path(cwd: &str, path: &str) -> String {
    // Per-process mount namespace root enforcement
    let ns_root = crate::process::with_current(|p| {
        if p.namespaces.mnt_ns == 0 { String::from("/") }
        else { crate::security::namespace::mnt_ns_root(p.namespaces.mnt_ns) }
    }).unwrap_or_else(|| String::from("/"));

    let raw = if path.starts_with('/') {
        if ns_root == "/" {
            canonicalize(path)
        } else {
            // Absolute path inside a non-root namespace: prepend namespace root
            let stripped = path.trim_start_matches('/');
            if stripped.is_empty() { ns_root.clone() }
            else { canonicalize(&alloc::format!("{}/{}", ns_root.trim_end_matches('/'), stripped)) }
        }
    } else {
        let base = if cwd.ends_with('/') {
            alloc::format!("{}{}", cwd, path)
        } else {
            alloc::format!("{}/{}", cwd, path)
        };
        canonicalize(&base)
    };

    // Security: ensure resolved path does not escape the namespace root
    if ns_root != "/" && !raw.starts_with(&ns_root) {
        // Path tried to escape (e.g. via ../../../../) — clamp to ns root
        return ns_root;
    }
    raw
}

fn canonicalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".."     => { parts.pop(); }
            c        => parts.push(c),
        }
    }
    if parts.is_empty() {
        String::from("/")
    } else {
        let mut s = String::new();
        for p in &parts { s.push('/'); s.push_str(p); }
        s
    }
}

const MAX_SYMLINK_DEPTH: usize = 40;

fn resolve_inode_inner(abs: &str, follow_last: bool, depth: usize) -> Result<Inode, VfsError> {
    if depth > MAX_SYMLINK_DEPTH { return Err(ELOOP); }
    let (sb, rel) = find_mount(abs).ok_or(ENOENT)?;
    let mut cur   = sb.ops.get_root()?;

    let parts: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    let last = parts.len().saturating_sub(1);

    for (i, &part) in parts.iter().enumerate() {
        let is_last = i == last;
        let child   = cur.ops.lookup(&cur, part)?;

        // Symlink handling
        if s_islnk(child.mode) && (follow_last || !is_last) {
            let target = child.ops.readlink(&child)?;
            let next_abs = if target.starts_with('/') {
                canonicalize(&target)
            } else {
                // Relative symlink: resolve relative to parent dir
                let parent = abs_path("/", &alloc::format!("{}/{}", {
                    let mut p = String::from(abs);
                    for _ in 0..parts.len() - i { let _ = p.rfind('/').map(|pos| p.truncate(pos)); }
                    p
                }, target));
                parent
            };
            return resolve_inode_inner(&next_abs, follow_last, depth + 1);
        }

        cur = child;
    }
    Ok(cur)
}

fn resolve_inode(cwd: &str, path: &str) -> Result<Inode, VfsError> {
    let abs = abs_path(cwd, path);
    resolve_inode_inner(&abs, true, 0)
}

fn resolve_inode_nofollow(cwd: &str, path: &str) -> Result<Inode, VfsError> {
    let abs = abs_path(cwd, path);
    resolve_inode_inner(&abs, false, 0)
}

// ── Public VFS API ────────────────────────────────────────────────────────

pub fn open(cwd: &str, path: &str, flags: i32, mode: u32) -> Result<FileDescriptor, VfsError> {
    let abs = abs_path(cwd, path);

    // Check for special /proc/sys paths
    if let Some(data) = crate::abi_compat::abi::handle_virtual_fs_path(&abs) {
        return Ok(make_mem_fd(data, flags));
    }

    let inode = match resolve_inode(cwd, path) {
        Ok(i) => i,
        Err(ENOENT) if flags & O_CREAT != 0 => {
            return create_file_at(cwd, path, mode);
        }
        Err(e) => return Err(e),
    };

    if flags & O_CREAT != 0 && flags & O_EXCL != 0 {
        return Err(EEXIST);
    }

    if flags & O_TRUNC != 0 && s_isreg(inode.mode) {
        inode.ops.truncate(&inode, 0)?;
    }

    let kind = fd_kind_for(&inode, &abs);
    let offset = if flags & O_APPEND != 0 { inode.size } else { 0 };
    Ok(FileDescriptor { inode, offset, flags, kind, path: abs })
}

fn fd_kind_for(inode: &Inode, abs: &str) -> FdKind {
    let name = abs.rsplit('/').next().unwrap_or("");
    match inode.mode & S_IFMT {
        S_IFDIR  => return FdKind::Directory,
        S_IFCHR  => {
            // DRM devices get their own kind for ioctl routing
            if name.starts_with("card") || name.starts_with("renderD") {
                return FdKind::Drm;
            }
            // Other char devices: extract minor from ops if available
            if let Some(minor) = inode.ops.device_minor() {
                return FdKind::Device(minor);
            }
        }
        _ => {}
    }
    FdKind::Regular
}

fn create_file_at(cwd: &str, path: &str, mode: u32) -> Result<FileDescriptor, VfsError> {
    let (parent_path, name) = split_path(path);
    let parent_abs = abs_path(cwd, &parent_path);
    let (sb, rel)  = find_mount(&parent_abs).ok_or(ENOENT)?;
    let parent_ino = {
        let mut cur = sb.ops.get_root()?;
        for part in rel.split('/').filter(|s| !s.is_empty()) {
            cur = cur.ops.lookup(&cur, part)?;
        }
        cur
    };
    let new_ino = parent_ino.ops.create(&parent_ino, &name, mode)?;
    let created_abs = abs_path(cwd, path);
    Ok(FileDescriptor { inode: new_ino, offset: 0, flags: O_RDWR,
                        kind: FdKind::Regular, path: created_abs })
}

fn make_mem_fd(data: Vec<u8>, flags: i32) -> FileDescriptor {
    use alloc::sync::Arc;
    struct MemOps { data: Vec<u8> }
    impl InodeOps for MemOps {
        fn read(&self, _: &Inode, buf: &mut [u8], off: u64) -> Result<usize, VfsError> {
            let s = off as usize;
            if s >= self.data.len() { return Ok(0); }
            let n = buf.len().min(self.data.len() - s);
            buf[..n].copy_from_slice(&self.data[s..s+n]);
            Ok(n)
        }
        fn write(&self, _:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EACCES)}
        fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{Err(ENOTDIR)}
        fn lookup(&self,_:&Inode,_:&str)->Result<Inode,VfsError>{Err(ENOENT)}
    }
    struct NSb; impl SuperblockOps for NSb { fn get_root(&self)->Result<Inode,VfsError>{Err(ENOENT)} }
    let sz = data.len() as u64;
    FileDescriptor {
        inode: Inode { ino: 0, mode: S_IFREG|0o444, uid:0, gid:0, size:sz,
            atime:0,mtime:0,ctime:0, ops:Arc::new(MemOps{data}),
            sb: Arc::new(Superblock{dev:99,fs_type:String::from("memfd"),ops:Arc::new(NSb)}) },
        offset: 0, flags, kind: FdKind::Regular, path: String::new(),
    }
}

pub fn stat(cwd: &str, path: &str) -> Result<Stat, VfsError> {
    Ok(resolve_inode(cwd, path)?.to_stat())
}

pub fn lstat(cwd: &str, path: &str) -> Result<Stat, VfsError> {
    Ok(resolve_inode_nofollow(cwd, path)?.to_stat())
}

pub fn fstat(fd: &FileDescriptor) -> Result<Stat, VfsError> {
    Ok(fd.inode.to_stat())
}

pub fn readlink(cwd: &str, path: &str) -> Result<String, VfsError> {
    let inode = resolve_inode_nofollow(cwd, path)?;
    if !s_islnk(inode.mode) { return Err(EINVAL); }
    inode.ops.readlink(&inode)
}

pub fn read_fd(fd: &FileDescriptor, buf: *mut u8, count: usize) -> Result<usize, VfsError> {
    match &fd.kind {
        // Directories cannot be read with read(2); use getdents64 instead.
        FdKind::Directory => return Err(EISDIR),
        FdKind::Regular => {
            let slice = unsafe { core::slice::from_raw_parts_mut(buf, count) };
            fd.inode.ops.read(&fd.inode, slice, fd.offset)
        }
        FdKind::PipeRead(pipe) => {
            let nonblock = fd.flags & O_NONBLOCK != 0;
            crate::ipc::pipe::pipe_read(pipe, buf, count, nonblock)
        }
        FdKind::Device(minor) => crate::device::read_device(*minor, buf, count, fd.offset),
        FdKind::Drm           => Ok(0),
        _                     => Err(EBADF),
    }
}

pub fn write_fd(fd: &FileDescriptor, buf: *const u8, count: usize) -> Result<usize, VfsError> {
    match &fd.kind {
        FdKind::Directory => return Err(EISDIR),
        FdKind::Regular => {
            let slice = unsafe { core::slice::from_raw_parts(buf, count) };
            fd.inode.ops.write(&fd.inode, slice, fd.offset)
        }
        FdKind::PipeWrite(pipe) => {
            let nonblock = fd.flags & O_NONBLOCK != 0;
            crate::ipc::pipe::pipe_write(pipe, buf, count, nonblock)
        }
        FdKind::Device(minor) => crate::device::write_device(*minor, buf, count, fd.offset),
        FdKind::Drm           => Ok(count),
        _                     => Err(EBADF),
    }
}

pub fn lseek(fd: &mut FileDescriptor, offset: i64, whence: i32) -> Result<u64, VfsError> {
    let new_off = match whence {
        0 => offset as u64,
        1 => (fd.offset as i64 + offset) as u64,
        2 => (fd.inode.size as i64 + offset) as u64,
        _ => return Err(EINVAL),
    };
    fd.offset = new_off;
    Ok(new_off)
}

/// Fill `buf` with `linux_dirent64` structs for the directory `fd`.
///
/// `fd.offset` is treated as an **entry count** (not byte offset): the
/// first `fd.offset` entries are skipped.  Returns the number of bytes
/// written; callers must advance `fd.offset` by the number of entries
/// consumed (use `getdents_entry_count` or the returned tuple variant).
///
/// Returns `(bytes_written, entries_consumed)` internally; the public
/// API returns only bytes so callers use `getdents_with_count`.
fn getdents_inner(fd: &FileDescriptor, buf: *mut u8, count: usize)
    -> Result<(usize, u64), VfsError>
{
    if !s_isdir(fd.inode.mode) { return Err(ENOTDIR); }

    // Always fetch all entries; skip the first fd.offset of them.
    // Passing 0 to readdir ensures all filesystems return from the start.
    let entries = fd.inode.ops.readdir(&fd.inode, 0)?;

    let skip = fd.offset as usize;
    let mut written    = 0usize;
    let mut n_consumed = 0u64;

    for entry in entries.iter().skip(skip) {
        let nlen   = entry.name.len();
        // linux_dirent64: ino(8) + off(8) + reclen(2) + type(1) + name + NUL
        // Total header = 19 bytes; align record to 8 bytes.
        let reclen = ((19 + nlen + 1 + 7) & !7).max(24);
        if written + reclen > count { break; }
        unsafe {
            let p = buf.add(written);
            // d_ino
            *(p as *mut u64)          = entry.ino;
            // d_off — the entry index of the NEXT entry (seek position)
            *(p.add(8)  as *mut i64)  = (skip as i64) + n_consumed as i64 + 1;
            // d_reclen
            *(p.add(16) as *mut u16)  = reclen as u16;
            // d_type
            *(p.add(18) as *mut u8)   = entry.file_type;
            // d_name + NUL
            core::ptr::copy_nonoverlapping(entry.name.as_ptr(), p.add(19), nlen);
            *p.add(19 + nlen) = 0u8;
        }
        written    += reclen;
        n_consumed += 1;
    }
    Ok((written, n_consumed))
}

/// Public `getdents` — used by sys_getdents64 via the handlers layer.
/// Returns only bytes written; callers must use the mut-offset variant.
pub fn getdents(fd: &FileDescriptor, buf: *mut u8, count: usize) -> Result<usize, VfsError> {
    getdents_inner(fd, buf, count).map(|(bytes, _)| bytes)
}

/// Public variant that also returns how many entries were consumed.
/// `sys_getdents64` uses this to advance `fd.offset` correctly.
pub fn getdents_and_advance(fd: &FileDescriptor, buf: *mut u8, count: usize)
    -> Result<(usize, u64), VfsError>
{
    getdents_inner(fd, buf, count)
}

pub fn resolve_dir(cwd: &str, path: &str) -> Result<String, VfsError> {
    let inode = resolve_inode(cwd, path)?;
    if !s_isdir(inode.mode) { return Err(ENOTDIR); }
    Ok(abs_path(cwd, path))
}

pub fn create_file(cwd: &str, path: &str, mode: u32) -> Result<FileDescriptor, VfsError> {
    create_file_at(cwd, path, mode)
}

pub fn mkdir(cwd: &str, path: &str, mode: u32) -> Result<(), VfsError> {
    let (parent, name) = split_path(path);
    let p_abs = abs_path(cwd, &parent);
    let (sb, rel) = find_mount(&p_abs).ok_or(ENOENT)?;
    let parent_ino = {
        let mut cur = sb.ops.get_root()?;
        for part in rel.split('/').filter(|s| !s.is_empty()) { cur = cur.ops.lookup(&cur, part)?; }
        cur
    };
    parent_ino.ops.mkdir(&parent_ino, &name, mode)?;
    Ok(())
}

pub fn unlink(cwd: &str, path: &str) -> Result<(), VfsError> {
    let (parent, name) = split_path(path);
    let p_abs = abs_path(cwd, &parent);
    let (sb, rel) = find_mount(&p_abs).ok_or(ENOENT)?;
    let parent_ino = {
        let mut cur = sb.ops.get_root()?;
        for part in rel.split('/').filter(|s| !s.is_empty()) { cur = cur.ops.lookup(&cur, part)?; }
        cur
    };
    parent_ino.ops.unlink(&parent_ino, &name)
}

pub fn rmdir(cwd: &str, path: &str) -> Result<(), VfsError> {
    let (parent, name) = split_path(path);
    let p_abs = abs_path(cwd, &parent);
    let (sb, rel) = find_mount(&p_abs).ok_or(ENOENT)?;
    let parent_ino = {
        let mut cur = sb.ops.get_root()?;
        for part in rel.split('/').filter(|s| !s.is_empty()) { cur = cur.ops.lookup(&cur, part)?; }
        cur
    };
    parent_ino.ops.rmdir(&parent_ino, &name)
}

pub fn symlink(cwd: &str, target: &str, link_path: &str) -> Result<(), VfsError> {
    let (parent, name) = split_path(link_path);
    let p_abs = abs_path(cwd, &parent);
    let (sb, rel) = find_mount(&p_abs).ok_or(ENOENT)?;
    let parent_ino = {
        let mut cur = sb.ops.get_root()?;
        for part in rel.split('/').filter(|s| !s.is_empty()) { cur = cur.ops.lookup(&cur, part)?; }
        cur
    };
    parent_ino.ops.symlink(&parent_ino, &name, target)?;
    Ok(())
}

pub fn rename(cwd: &str, old: &str, new: &str) -> Result<(), VfsError> {
    let (old_parent_path, old_name) = split_path(old);
    let (new_parent_path, new_name) = split_path(new);

    let old_parent_abs = abs_path(cwd, &old_parent_path);
    let new_parent_abs = abs_path(cwd, &new_parent_path);

    if old_parent_abs != new_parent_abs {
        // Cross-directory rename: not supported by tmpfs currently
        return Err(EXDEV);
    }

    let (sb, rel) = find_mount(&old_parent_abs).ok_or(ENOENT)?;
    let parent_ino = {
        let mut cur = sb.ops.get_root()?;
        for part in rel.split('/').filter(|s| !s.is_empty()) { cur = cur.ops.lookup(&cur, part)?; }
        cur
    };

    // Get new parent ino for cross-dir (same here since we checked == above)
    parent_ino.ops.rename(&parent_ino, &old_name, parent_ino.ino, &new_name)
}

/// Read the complete contents of an open file into `out`.
///
/// Uses `inode.size` as the initial hint, but falls back to a chunked
/// read loop for files where `size` is 0 or stale (e.g. procfs, pipes).
pub fn read_all_fd(fd: &FileDescriptor, out: &mut Vec<u8>) {
    const CHUNK: usize = 4096;

    let hint = fd.inode.size as usize;

    if hint > 0 {
        // Normal case: size is known — read in one shot
        out.resize(hint, 0);
        match fd.inode.ops.read(&fd.inode, out, 0) {
            Ok(n) => { out.truncate(n); }
            Err(_) => { out.clear(); }
        }
        return;
    }

    // Size unknown (e.g. procfs virtual file, pipe) — chunked read
    let mut offset = 0u64;
    loop {
        let old_len = out.len();
        out.resize(old_len + CHUNK, 0);
        match fd.inode.ops.read(&fd.inode, &mut out[old_len..], offset) {
            Ok(0) => {
                out.truncate(old_len); // EOF
                break;
            }
            Ok(n) => {
                out.truncate(old_len + n);
                offset += n as u64;
                if n < CHUNK { break; } // likely EOF on next read
            }
            Err(_) => {
                out.truncate(old_len);
                break;
            }
        }
        // Safety cap: 256 MB max
        if out.len() > 256 * 1024 * 1024 { break; }
    }
}

fn split_path(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(0) | None if path.starts_with('/') => (String::from("/"), path[1..].to_owned()),
        Some(pos) => {
            let parent = &path[..pos];
            (if parent.is_empty() { String::from("/") } else { parent.to_owned() },
             path[pos+1..].to_owned())
        }
        None => (String::from("."), path.to_owned()),
    }
}

pub fn sync_all() {
    let m = MOUNTS.lock();
    for mp in m.iter() { mp.sb.ops.sync(); }
}

pub fn chmod_path(cwd: &str, path: &str, mode: u32) -> Result<(), VfsError> {
    let inode = resolve_inode(cwd, path)?;
    inode.ops.chmod(&inode, mode)
}

pub fn stat_path(cwd: &str, path: &str) -> Result<Inode, VfsError> {
    resolve_inode(cwd, path)
}

pub fn rmdir_path(cwd: &str, path: &str) -> Result<(), VfsError> {
    let (parent, name) = split_path(path);
    let parent_abs = abs_path(cwd, &parent);
    let parent_inode = resolve_inode_inner(&parent_abs, true, 0)?;
    parent_inode.ops.rmdir(&parent_inode, &name)
}

// Plugin fs hooks available via crate::plugins::hooks::fs_operation()

pub const ENOBUFS: u32 = 105;

pub const EADDRINUSE: u32 = 98;

pub const ECONNREFUSED: u32 = 111;

pub const ENOTCONN: u32 = 107;

pub const EISCONN: u32 = 106;

pub const ETIMEDOUT: u32 = 110;

// Dummy implementations used when a real filesystem isn't needed
pub struct DummyInodeOps;
impl DummyInodeOps { pub fn new() -> alloc::sync::Arc<dyn InodeOps> { alloc::sync::Arc::new(DummyInodeOps) } }
impl InodeOps for DummyInodeOps {
    fn read(&self, _: &Inode, _: &mut [u8], _: u64) -> Result<usize, VfsError> { Err(EBADF) }
    fn write(&self, _: &Inode, _: &[u8], _: u64) -> Result<usize, VfsError> { Err(EBADF) }
    fn readdir(&self, _: &Inode, _: u64) -> Result<alloc::vec::Vec<DirEntry>, VfsError> { Err(ENOTDIR) }
    fn lookup(&self, _: &Inode, _: &str) -> Result<Inode, VfsError> { Err(ENOENT) }
}
pub struct DummySuperblock;
impl DummySuperblock { pub fn new() -> alloc::sync::Arc<dyn SuperblockOps> { alloc::sync::Arc::new(DummySuperblock) } }
impl SuperblockOps for DummySuperblock {
    fn get_root(&self) -> Result<Inode, VfsError> { Err(ENOENT) }
}
