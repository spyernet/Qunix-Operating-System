//! Anonymous pipes — classic Unix read/write end pairs backed by a ring buffer.
//!
//! ## Design
//!
//! Each pipe has one `PipeBuf` behind an `Arc<Mutex<PipeBuf>>`.  Both the
//! read end and write end hold a clone of that Arc.
//!
//! Reference counts (`readers` / `writers`) are incremented by `dup_read` /
//! `dup_write` (called when an fd is dup'd) and decremented by
//! `close_read` / `close_write` (called by `sys_close`).
//!
//! A global wait-queue maps pipe pointers to lists of blocked PIDs.
//! `read` / `write` register the caller before sleeping; whoever
//! makes progress wakes the appropriate waiters.

use alloc::sync::Arc;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use spin::Mutex;
use crate::vfs::{FileDescriptor, FdKind, Inode, InodeOps, Superblock,
                  DirEntry, VfsError, S_IFIFO, EAGAIN, ENOENT, EINVAL, EPIPE};

pub const PIPE_CAPACITY: usize = 65536;

// ── Global pipe wait-queue ─────────────────────────────────────────────────
//
// Key  = raw pointer value of the PipeBuf (stable because it lives behind Arc).
// Value = (readers_waiting, writers_waiting)
//
// We use a simple spinlock-protected BTreeMap.  The pointer is usize so
// it is Send + Ord.

type WaitMap = BTreeMap<usize, (Vec<crate::process::Pid>, Vec<crate::process::Pid>)>;
static PIPE_WAITERS: Mutex<WaitMap> = Mutex::new(BTreeMap::new());

fn pipe_key(pipe: &Arc<Mutex<PipeBuf>>) -> usize {
    // The raw pointer to the Mutex<PipeBuf> allocation — stable for the
    // lifetime of the Arc.
    Arc::as_ptr(pipe) as usize
}

fn register_reader_waiter(pipe: &Arc<Mutex<PipeBuf>>, pid: crate::process::Pid) {
    let k = pipe_key(pipe);
    let mut wq = PIPE_WAITERS.lock();
    wq.entry(k).or_default().0.push(pid);
}

/// Public alias for poll() — registers `pid` as a reader waiter without blocking.
pub fn register_reader_waiter_pub(pipe: &Arc<Mutex<PipeBuf>>, pid: crate::process::Pid) {
    register_reader_waiter(pipe, pid);
}

fn register_writer_waiter(pipe: &Arc<Mutex<PipeBuf>>, pid: crate::process::Pid) {
    let k = pipe_key(pipe);
    let mut wq = PIPE_WAITERS.lock();
    wq.entry(k).or_default().1.push(pid);
}

/// Public alias for poll() — registers `pid` as a writer waiter without blocking.
pub fn register_writer_waiter_pub(pipe: &Arc<Mutex<PipeBuf>>, pid: crate::process::Pid) {
    register_writer_waiter(pipe, pid);
}

fn wake_readers(pipe: &Arc<Mutex<PipeBuf>>) {
    let k = pipe_key(pipe);
    let pids = {
        let mut wq = PIPE_WAITERS.lock();
        if let Some(entry) = wq.get_mut(&k) {
            core::mem::take(&mut entry.0)
        } else {
            Vec::new()
        }
    };
    for pid in pids { crate::sched::wake_process(pid); }
}

fn wake_writers(pipe: &Arc<Mutex<PipeBuf>>) {
    let k = pipe_key(pipe);
    let pids = {
        let mut wq = PIPE_WAITERS.lock();
        if let Some(entry) = wq.get_mut(&k) {
            core::mem::take(&mut entry.1)
        } else {
            Vec::new()
        }
    };
    for pid in pids { crate::sched::wake_process(pid); }
}

fn cleanup_waiters(pipe: &Arc<Mutex<PipeBuf>>) {
    let k = pipe_key(pipe);
    PIPE_WAITERS.lock().remove(&k);
}

// ── Ring-buffer kernel pipe ────────────────────────────────────────────────

pub struct PipeBuf {
    pub buf:          Vec<u8>,
    pub read_pos:     usize,
    pub write_pos:    usize,
    pub len:          usize,
    pub write_closed: bool,
    pub read_closed:  bool,
    pub readers:      usize,
    pub writers:      usize,
}

impl PipeBuf {
    pub fn new() -> Self {
        PipeBuf {
            buf:          alloc::vec![0u8; PIPE_CAPACITY],
            read_pos:     0,
            write_pos:    0,
            len:          0,
            write_closed: false,
            read_closed:  false,
            readers:      1,
            writers:      1,
        }
    }

    pub fn readable(&self) -> bool { self.len > 0 || self.write_closed }
    pub fn writable(&self) -> bool { self.len < PIPE_CAPACITY && !self.read_closed }

    /// Non-blocking read.  Returns:
    ///   Ok(n)       — n bytes read (n may be 0 if write end closed = EOF)
    ///   Err(EAGAIN) — no data yet, caller should block
    ///   Err(EPIPE)  — should not happen on read end
    pub fn try_read(&mut self, buf: &mut [u8]) -> Result<usize, VfsError> {
        if self.len == 0 {
            return if self.write_closed { Ok(0) } else { Err(EAGAIN) };
        }
        let n = buf.len().min(self.len);
        for i in 0..n {
            buf[i] = self.buf[self.read_pos];
            self.read_pos = (self.read_pos + 1) % PIPE_CAPACITY;
        }
        self.len -= n;
        Ok(n)
    }

    /// Non-blocking write.  Returns:
    ///   Ok(n)       — n bytes written
    ///   Err(EPIPE)  — all readers closed
    ///   Err(EAGAIN) — buffer full, caller should block
    pub fn try_write(&mut self, data: &[u8]) -> Result<usize, VfsError> {
        if self.read_closed {
            return Err(EPIPE);
        }
        let space = PIPE_CAPACITY - self.len;
        if space == 0 { return Err(EAGAIN); }
        let n = data.len().min(space);
        for i in 0..n {
            self.buf[self.write_pos] = data[i];
            self.write_pos = (self.write_pos + 1) % PIPE_CAPACITY;
        }
        self.len += n;
        Ok(n)
    }

    pub fn close_write(&mut self) {
        debug_assert!(self.writers > 0,
            "pipe_close_write: writers already 0 (double-close bug)");
        if self.writers > 0 { self.writers -= 1; }
        if self.writers == 0 { self.write_closed = true; }
    }

    pub fn close_read(&mut self) {
        debug_assert!(self.readers > 0,
            "pipe_close_read: readers already 0 (double-close bug)");
        if self.readers > 0 { self.readers -= 1; }
        if self.readers == 0 { self.read_closed = true; }
    }

    pub fn dup_write(&mut self) {
        self.writers = self.writers.saturating_add(1);
        debug_assert!(!self.write_closed,
            "pipe dup_write on already-closed write end");
    }
    pub fn dup_read(&mut self) {
        self.readers = self.readers.saturating_add(1);
        debug_assert!(!self.read_closed,
            "pipe dup_read on already-closed read end");
    }
}

// ── Blocking read/write (called from sys_read / sys_write) ────────────────

/// Blocking pipe read.  Parks the current process until data is available
/// or all writers have closed.
pub fn pipe_read(
    pipe: &Arc<Mutex<PipeBuf>>,
    buf:  *mut u8,
    count: usize,
    nonblock: bool,
) -> Result<usize, VfsError> {
    let slice = unsafe { core::slice::from_raw_parts_mut(buf, count) };

    loop {
        let result = pipe.lock().try_read(slice);
        match result {
            Ok(n) => {
                // Woke a writer: space freed in buffer
                if n > 0 { wake_writers(pipe); }
                return Ok(n);
            }
            Err(EAGAIN) => {
                if nonblock { return Err(EAGAIN); }
                // Register as waiter, then block
                let pid = crate::process::current_pid();
                register_reader_waiter(pipe, pid);
                crate::sched::block_current(crate::process::ProcessState::Sleeping);
                // Woken by a writer — retry
            }
            Err(e) => return Err(e),
        }
    }
}

/// Blocking pipe write.  Parks until there is space in the buffer.
/// Sends SIGPIPE and returns EPIPE if all readers are closed.
pub fn pipe_write(
    pipe:  &Arc<Mutex<PipeBuf>>,
    buf:   *const u8,
    count: usize,
    nonblock: bool,
) -> Result<usize, VfsError> {
    let slice = unsafe { core::slice::from_raw_parts(buf, count) };
    let mut written = 0usize;

    while written < count {
        let result = pipe.lock().try_write(&slice[written..]);
        match result {
            Ok(n) => {
                written += n;
                // Wake any blocked readers
                wake_readers(pipe);
            }
            Err(EPIPE) => {
                // All readers gone — send SIGPIPE to self
                crate::signal::send_signal(
                    crate::process::current_pid(),
                    crate::signal::SIGPIPE,
                );
                return if written > 0 { Ok(written) } else { Err(EPIPE) };
            }
            Err(EAGAIN) => {
                if nonblock {
                    return if written > 0 { Ok(written) } else { Err(EAGAIN) };
                }
                // Register as waiter, then block until a reader frees space
                let pid = crate::process::current_pid();
                register_writer_waiter(pipe, pid);
                crate::sched::block_current(crate::process::ProcessState::Sleeping);
                // Woken by a reader — retry
            }
            Err(e) => return Err(e),
        }
    }
    Ok(written)
}

/// Called when the last write-end Arc is dropped or explicitly closed.
/// Wakes all blocked readers so they see EOF.
pub fn pipe_close_write(pipe: &Arc<Mutex<PipeBuf>>) {
    pipe.lock().close_write();
    wake_readers(pipe);
    // If both ends are now closed, clean up waiters entry
    let both_closed = {
        let g = pipe.lock();
        g.write_closed && g.read_closed
    };
    if both_closed { cleanup_waiters(pipe); }
}

/// Called when the last read-end Arc is dropped or explicitly closed.
/// Wakes all blocked writers so they get SIGPIPE/EPIPE.
pub fn pipe_close_read(pipe: &Arc<Mutex<PipeBuf>>) {
    pipe.lock().close_read();
    wake_writers(pipe);
    let both_closed = {
        let g = pipe.lock();
        g.write_closed && g.read_closed
    };
    if both_closed { cleanup_waiters(pipe); }
}

// ── Null inode ops for pipe fd ─────────────────────────────────────────────

struct PipeInodeOps;
impl InodeOps for PipeInodeOps {
    fn read(&self,    _: &Inode, _: &mut [u8], _: u64) -> Result<usize, VfsError> { Err(EINVAL) }
    fn write(&self,   _: &Inode, _: &[u8],     _: u64) -> Result<usize, VfsError> { Err(EINVAL) }
    fn readdir(&self, _: &Inode, _: u64) -> Result<Vec<DirEntry>, VfsError>        { Err(EINVAL) }
    fn lookup(&self,  _: &Inode, _: &str) -> Result<Inode, VfsError>              { Err(ENOENT) }
}

struct PipeSbOps;
impl crate::vfs::SuperblockOps for PipeSbOps {
    fn get_root(&self) -> Result<Inode, VfsError> { Err(ENOENT) }
}

static PIPE_INO: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0x8000_0000);

fn make_pipe_inode() -> Inode {
    Inode {
        ino:  PIPE_INO.fetch_add(1, core::sync::atomic::Ordering::Relaxed),
        mode: S_IFIFO | 0o600,
        uid: 0, gid: 0, size: 0,
        atime: 0, mtime: 0, ctime: 0,
        ops: Arc::new(PipeInodeOps),
        sb:  Arc::new(Superblock {
            dev: 0,
            fs_type: alloc::string::String::from("pipefs"),
            ops: Arc::new(PipeSbOps),
        }),
    }
}

// ── Public constructors ────────────────────────────────────────────────────

pub fn new_pipe() -> (FileDescriptor, FileDescriptor) {
    let buf = Arc::new(Mutex::new(PipeBuf::new()));
    let rfd = FileDescriptor {
        inode:  make_pipe_inode(),
        offset: 0, flags: 0,
        kind:   FdKind::PipeRead(buf.clone()),
        path:   alloc::string::String::new(),
    };
    let wfd = FileDescriptor {
        inode:  make_pipe_inode(),
        offset: 0, flags: 0,
        kind:   FdKind::PipeWrite(buf),
        path:   alloc::string::String::new(),
    };
    (rfd, wfd)
}

pub fn new_pipe2(flags: i32) -> (FileDescriptor, FileDescriptor) {
    let (mut r, mut w) = new_pipe();
    if flags & 0o4000    != 0 { r.flags |= 0o4000;    w.flags |= 0o4000;    }
    if flags & 0o2000000 != 0 { r.flags |= 0o2000000; w.flags |= 0o2000000; }
    (r, w)
}
