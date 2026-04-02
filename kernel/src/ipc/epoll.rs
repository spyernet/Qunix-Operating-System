use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;

pub const EPOLLIN:  u32 = 0x001;
pub const EPOLLOUT: u32 = 0x004;
pub const EPOLLERR: u32 = 0x008;
pub const EPOLLHUP: u32 = 0x010;
pub const EPOLLRDHUP: u32 = 0x2000;
pub const EPOLLET:  u32 = 1 << 31;
pub const EPOLLONESHOT: u32 = 1 << 30;

pub const EPOLL_CTL_ADD: i32 = 1;
pub const EPOLL_CTL_DEL: i32 = 2;
pub const EPOLL_CTL_MOD: i32 = 3;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct EpollEvent {
    pub events: u32,
    pub data:   u64,
}

struct EpollInstance {
    watched: BTreeMap<i32, (u32, u64)>,  // fd -> (events mask, user data)
}

static EPOLL_TABLE: Mutex<BTreeMap<u32, EpollInstance>> = Mutex::new(BTreeMap::new());
static NEXT_EPOLL_FD: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(200);

pub fn epoll_create(flags: i32) -> i64 {
    use crate::vfs::{FileDescriptor, FdKind, Inode, InodeOps, DirEntry, VfsError};
    use alloc::sync::Arc;
    use alloc::string::String;

    let epfd = NEXT_EPOLL_FD.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    EPOLL_TABLE.lock().insert(epfd, EpollInstance { watched: BTreeMap::new() });

    struct EpollIno { epfd: u32 }
    impl InodeOps for EpollIno {
        fn read(&self, _: &Inode, _: &mut [u8], _: u64) -> Result<usize, VfsError> { Ok(0) }
        fn write(&self, _: &Inode, _: &[u8], _: u64) -> Result<usize, VfsError> { Ok(0) }
        fn readdir(&self, _: &Inode, _: u64) -> Result<Vec<DirEntry>, VfsError> { Err(crate::vfs::ENOTDIR) }
        fn lookup(&self, _: &Inode, _: &str) -> Result<Inode, VfsError> { Err(crate::vfs::ENOENT) }
    }
    struct NullSb;
    impl crate::vfs::SuperblockOps for NullSb {
        fn get_root(&self) -> Result<Inode, VfsError> { Err(crate::vfs::ENOENT) }
    }

    let inode = Inode {
        ino: 0xE000_0000 + epfd as u64,
        mode: crate::vfs::S_IFREG | 0o600,
        uid: 0, gid: 0, size: 0,
        atime: 0, mtime: 0, ctime: 0,
        ops: Arc::new(EpollIno { epfd }),
        sb: Arc::new(crate::vfs::Superblock {
            dev: 11, fs_type: String::from("epollfs"), ops: Arc::new(NullSb),
        }),
    };
    let fd_obj = FileDescriptor { inode, offset: 0, flags: 0, kind: FdKind::Regular, path: alloc::string::String::new(),};
    crate::process::with_current_mut(|p| p.alloc_fd(fd_obj)).map(|n| n as i64).unwrap_or(-9)
}

pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: u64) -> i64 {
    let ep_id = (epfd as u32).wrapping_sub(0); // epfd is the kernel fd
    // Find which epoll instance this kernel fd maps to
    let ep_instance_id = find_epoll_instance(epfd);
    let ep_instance_id = match ep_instance_id {
        Some(id) => id,
        None => return -9,
    };

    let ev = if event != 0 {
        unsafe { *(event as *const EpollEvent) }
    } else {
        EpollEvent { events: 0, data: 0 }
    };

    let mut table = EPOLL_TABLE.lock();
    let instance = match table.get_mut(&ep_instance_id) {
        Some(i) => i,
        None => return -9,
    };

    match op {
        EPOLL_CTL_ADD => {
            instance.watched.insert(fd, (ev.events, ev.data));
            0
        }
        EPOLL_CTL_DEL => {
            instance.watched.remove(&fd);
            0
        }
        EPOLL_CTL_MOD => {
            if let Some(entry) = instance.watched.get_mut(&fd) {
                *entry = (ev.events, ev.data);
                0
            } else { -2 }
        }
        _ => -22,
    }
}

pub fn epoll_wait(epfd: i32, events_ptr: u64, maxevents: i32, timeout_ms: i32) -> i64 {
    let ep_instance_id = match find_epoll_instance(epfd) {
        Some(id) => id,
        None => return -9,
    };

    // Simplified: poll all watched fds, return readable ones immediately
    let watched: Vec<(i32, u32, u64)> = {
        let table = EPOLL_TABLE.lock();
        match table.get(&ep_instance_id) {
            Some(inst) => inst.watched.iter()
                .map(|(&fd, &(evs, data))| (fd, evs, data))
                .collect(),
            None => return -9,
        }
    };

    let mut count = 0i32;
    let max = maxevents.min(1024) as usize;

    for (fd, mask, data) in &watched {
        if count >= maxevents { break; }
        let ready = check_fd_ready(*fd, *mask);
        if ready != 0 {
            let out_ev = EpollEvent { events: ready, data: *data };
            unsafe {
                *((events_ptr + count as u64 * 12) as *mut EpollEvent) = out_ev;
            }
            count += 1;
        }
    }

    if count == 0 && timeout_ms != 0 {
        // Block briefly if no events and timeout requested
        if timeout_ms > 0 {
            crate::time::sleep_ticks(timeout_ms.min(100) as u64);
        }
    }

    count as i64
}

fn check_fd_ready(fd: i32, mask: u32) -> u32 {
    // Check if fd has data available
    let has_data = crate::process::with_current(|p| {
        p.get_fd(fd as u32).map(|f| {
            match &f.kind {
                crate::vfs::FdKind::PipeRead(pipe) => pipe.lock().len > 0,
                _ => true, // regular files always ready
            }
        }).unwrap_or(false)
    });

    if has_data.unwrap_or(false) && mask & EPOLLIN != 0 { EPOLLIN }
    else if mask & EPOLLOUT != 0 { EPOLLOUT }
    else { 0 }
}

fn find_epoll_instance(epfd: i32) -> Option<u32> {
    // The epfd is a kernel fd; we stored the epoll instance ID starting at NEXT_EPOLL_FD-origin
    // Check all epoll instances by looking at inode ino encoding
    let ino = crate::process::with_current(|p| {
        p.get_fd(epfd as u32).map(|f| f.inode.ino)
    });
    ino.flatten().and_then(|iv| {
        if iv >= 0xE000_0000 { Some((iv - 0xE000_0000) as u32) } else { None }
    })
}

// Allow pipe's PipeBuf to expose recv_buf for epoll check
pub trait HasRecvBuf {
    fn recv_buf_len(&self) -> usize;
}
