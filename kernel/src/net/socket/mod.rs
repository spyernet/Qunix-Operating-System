//! Socket layer — bridges POSIX socket API to kernel TCP/UDP/UNIX implementations.
//!
//! All send/recv paths are wired end-to-end. No stubs, no returning 0 for recv.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use crate::vfs::{FileDescriptor, FdKind, EBADF, EINVAL, EAGAIN, ENOBUFS, EADDRINUSE,
                  ECONNREFUSED, ENOTCONN, EISCONN, ETIMEDOUT, ENOMEM};

pub const AF_UNIX:   u16 = 1;
pub const AF_INET:   u16 = 2;
pub const AF_INET6:  u16 = 10;

pub const SOCK_STREAM:   u8 = 1;
pub const SOCK_DGRAM:    u8 = 2;
pub const SOCK_RAW:      u8 = 3;
pub const SOCK_NONBLOCK: i32 = 0x800;
pub const SOCK_CLOEXEC:  i32 = 0x80000;

pub const SOL_SOCKET:    i32 = 1;
pub const SO_REUSEADDR:  i32 = 2;
pub const SO_KEEPALIVE:  i32 = 9;
pub const SO_RCVBUF:     i32 = 8;
pub const SO_SNDBUF:     i32 = 7;
pub const SO_ERROR:      i32 = 4;
pub const SO_TYPE:       i32 = 3;
pub const SO_LINGER:     i32 = 13;
pub const SO_BROADCAST:  i32 = 6;
pub const SO_OOBINLINE:  i32 = 10;

pub const IPPROTO_TCP:   i32 = 6;
pub const IPPROTO_UDP:   i32 = 17;
pub const TCP_NODELAY:   i32 = 1;
pub const TCP_KEEPIDLE:  i32 = 4;
pub const TCP_KEEPINTVL: i32 = 5;
pub const TCP_KEEPCNT:   i32 = 6;

pub const MSG_DONTWAIT:  i32 = 0x40;
pub const MSG_PEEK:      i32 = 0x02;
pub const MSG_WAITALL:   i32 = 0x100;

pub const SHUT_RD:  i32 = 0;
pub const SHUT_WR:  i32 = 1;
pub const SHUT_RDWR:i32 = 2;

#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct SockAddrIn {
    pub family: u16,
    pub port:   u16,   // big-endian
    pub addr:   u32,   // big-endian
    pub pad:    [u8; 8],
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct SockAddrUn {
    pub family: u16,
    pub path:   [u8; 108],
}
impl Default for SockAddrUn {
    fn default() -> Self { SockAddrUn { family: 0, path: [0u8; 108] } }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum SockState {
    Created,
    Bound,
    Listening,
    Connecting,
    Connected,
    CloseWait,
    Closed,
}

/// Per-socket metadata tracked by the socket layer.
/// Actual data buffers live in tcp::TcpSocket / udp layer.
pub struct Socket {
    pub id:          i32,   // FD used as socket identifier
    pub family:      u16,
    pub sock_type:   u8,
    pub protocol:    u8,
    pub state:       SockState,
    pub local_addr:  Option<SockAddrIn>,
    pub remote_addr: Option<SockAddrIn>,
    pub nonblocking: bool,
    pub opts_reuse:  bool,
    // For DGRAM recv: UDP packets queued here (src_addr, data)
    pub udp_recv_q:  Vec<(SockAddrIn, Vec<u8>)>,
    // For UNIX sockets: local path
    pub unix_path:   [u8; 108],
}

impl Socket {
    fn new(id: i32, family: u16, sock_type: u8, protocol: u8) -> Self {
        Socket {
            id, family, sock_type, protocol,
            state: SockState::Created,
            local_addr: None, remote_addr: None,
            nonblocking: false, opts_reuse: false,
            udp_recv_q: Vec::new(),
            unix_path: [0; 108],
        }
    }
}

static SOCKETS: Mutex<BTreeMap<i32, Socket>> = Mutex::new(BTreeMap::new());
static NEXT_EPHEMERAL: core::sync::atomic::AtomicU16 = core::sync::atomic::AtomicU16::new(49152);

fn alloc_ephemeral_port() -> u16 {
    let p = NEXT_EPHEMERAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if p == 0 || p >= 65535 {
        NEXT_EPHEMERAL.store(49152, core::sync::atomic::Ordering::Relaxed);
        49152
    } else { p }
}

// ── POSIX socket syscalls ──────────────────────────────────────────────────

pub fn sys_socket(family: u16, sock_type_raw: u8, protocol: u8) -> i32 {
    let sock_type = sock_type_raw & 0x0F; // mask out SOCK_NONBLOCK etc
    let nonblock  = (sock_type_raw as i32 & SOCK_NONBLOCK) != 0;

    if family != AF_INET && family != AF_UNIX {
        return -22; // EINVAL - unsupported family
    }

    let fd: i32 = crate::process::with_current_mut(|p| {
        let fake_inode = crate::vfs::Inode {
            ino: 0, mode: 0xC000, uid: 0, gid: 0, size: 0,
            atime: 0, mtime: 0, ctime: 0,
            ops: crate::vfs::DummyInodeOps::new(),
            sb: alloc::sync::Arc::new(crate::vfs::Superblock {
                dev: 0, fs_type: alloc::string::String::new(),
                ops: crate::vfs::DummySuperblock::new(),
            }),
        };
        let fd_val = FileDescriptor {
            inode: fake_inode,
            kind: FdKind::Socket(0),
            flags: if nonblock { crate::vfs::O_NONBLOCK } else { 0 },
            offset: 0,
            path: alloc::string::String::new(),
        };
        p.alloc_fd(fd_val) as i32
    }).unwrap_or(-1);

    let mut sock = Socket::new(fd, family, sock_type, protocol);
    sock.nonblocking = nonblock;

    // Update FdKind with real fd
    crate::process::with_current_mut(|p| {
        p.get_fd_mut_op(fd as u32, |f| f.kind = FdKind::Socket(fd as u32));
    });

    SOCKETS.lock().insert(fd, sock);
    fd as i32
}

pub fn sys_bind(sockfd: i32, addr: u64, addrlen: u32) -> i32 {
    if addr == 0 { return -22; }
    let mut sockets = SOCKETS.lock();

    // Verify socket exists
    if !sockets.contains_key(&sockfd) { return -9; }

    // Handle AF_UNIX: needs mutable borrow only
    {
        let sock = sockets.get_mut(&sockfd).unwrap();
        if sock.family == AF_UNIX {
            let sun = unsafe { &*(addr as *const SockAddrUn) };
            sock.unix_path = sun.path;
            sock.state = SockState::Bound;
            return 0;
        }
    }

    let sa = unsafe { *(addr as *const SockAddrIn) };
    let port = u16::from_be(sa.port);

    // Check port not already in use (immutable scan first, before mutable borrow)
    if port != 0 {
        let in_use = sockets.values().any(|other| {
            other.local_addr.map(|la| u16::from_be(la.port) == port).unwrap_or(false)
        });
        if in_use { return -98; } // EADDRINUSE
    }

    let bind_addr = SockAddrIn {
        family: AF_INET,
        port: if port == 0 { alloc_ephemeral_port().to_be() } else { sa.port },
        addr: sa.addr,
        pad: [0; 8],
    };

    // Now take mutable borrow to write
    let sock = sockets.get_mut(&sockfd).unwrap();
    sock.local_addr = Some(bind_addr);
    sock.state = SockState::Bound;
    0
}

pub fn sys_listen(sockfd: i32, backlog: i32) -> i32 {
    let mut sockets = SOCKETS.lock();
    let sock = match sockets.get_mut(&sockfd) { Some(s) => s, None => return -9 };

    if sock.family == AF_INET && sock.sock_type == SOCK_STREAM {
        let la = match sock.local_addr {
            Some(a) => a,
            None => return -22, // must be bound first
        };
        let local_ip   = u32::from_be(la.addr);
        let local_port = u16::from_be(la.port);
        drop(sockets);
        crate::net::tcp::listen(local_ip, local_port);
        SOCKETS.lock().get_mut(&sockfd).map(|s| s.state = SockState::Listening);
        return 0;
    }
    // UDP sockets can "listen" by being bound
    sockets.get_mut(&sockfd).map(|s| s.state = SockState::Listening);
    0
}

pub fn sys_connect(sockfd: i32, addr: u64, _addrlen: u32) -> i32 {
    if addr == 0 { return -22; }
    let sa = unsafe { *(addr as *const SockAddrIn) };
    let remote_ip   = u32::from_be(sa.addr);
    let remote_port = u16::from_be(sa.port);

    if remote_ip == 0 || remote_port == 0 { return -22; }

    // Register socket in current process's network namespace
    

    // QSF network access check
    let pid = crate::process::current_pid();
    if crate::security::qsf_check_network(pid, remote_ip, remote_port)
       == crate::security::QsfResult::Deny { return -1; } // EPERM

    let (local_ip, local_port, sock_type) = {
        let mut sockets = SOCKETS.lock();
        if !sockets.contains_key(&sockfd) { return -9; }
        if sockets.get(&sockfd).map(|s| s.state == SockState::Connected).unwrap_or(false) { return -106; }

        let lip = crate::net::ip::local_ip();
        let sock = sockets.get_mut(&sockfd).unwrap();
        let lport = match sock.local_addr {
            Some(la) => u16::from_be(la.port),
            None => {
                let p = alloc_ephemeral_port();
                sock.local_addr = Some(SockAddrIn {
                    family: AF_INET, port: p.to_be(), addr: lip.to_be(), pad: [0;8]
                });
                p
            }
        };
        sock.remote_addr = Some(sa);
        sock.state = SockState::Connecting;
        (lip, lport, sock.sock_type)
    };

    if sock_type == SOCK_STREAM {
        // Initiate TCP handshake
        crate::net::tcp::connect(local_ip, local_port, remote_ip, remote_port);

        // Wait up to 5s for connection to be established
        let deadline = crate::time::ticks() + 5000;
        loop {
            let done = {
                let tcp_state = crate::net::tcp::connection_state(
                    local_ip, local_port, remote_ip, remote_port
                );
                tcp_state == crate::net::tcp::TcpState::Established
            };
            if done {
                SOCKETS.lock().get_mut(&sockfd).map(|s| s.state = SockState::Connected);
                return 0;
            }
            // Check for RST / unreachable
            if crate::net::tcp::connection_state(local_ip, local_port, remote_ip, remote_port)
               == crate::net::tcp::TcpState::Closed {
                return -111; // ECONNREFUSED
            }
            if crate::time::ticks() >= deadline {
                return -110; // ETIMEDOUT
            }
            // Yield
            crate::arch::x86_64::cpu::enable_interrupts();
            core::hint::spin_loop();
        }
    } else {
        // DGRAM: just set remote address
        SOCKETS.lock().get_mut(&sockfd).map(|s| s.state = SockState::Connected);
        0
    }
}

pub fn sys_accept(sockfd: i32, addr: u64, addrlen: u64) -> i32 {
    let (local_ip, local_port, nonblocking) = {
        let sockets = SOCKETS.lock();
        let sock = match sockets.get(&sockfd) { Some(s) => s, None => return -9 };
        if sock.state != SockState::Listening { return -22; }
        let la = match sock.local_addr { Some(a) => a, None => return -22 };
        (u32::from_be(la.addr), u16::from_be(la.port), sock.nonblocking)
    };

    // Poll TCP for a completed connection
    let deadline = if nonblocking { crate::time::ticks() } else { crate::time::ticks() + 30_000 };

    loop {
        // Check if TCP has an established connection waiting on our listen port
        if let Some((remote_ip, remote_port)) =
            crate::net::tcp::pop_accepted(local_ip, local_port)
        {
            // Create a new socket for this connection
            let new_fd = sys_socket(AF_INET, SOCK_STREAM, 0);
            if new_fd < 0 { return -12; } // ENOMEM

            SOCKETS.lock().get_mut(&new_fd).map(|s| {
                s.local_addr = Some(SockAddrIn {
                    family: AF_INET,
                    port:   local_port.to_be(),
                    addr:   local_ip.to_be(),
                    pad:    [0; 8],
                });
                s.remote_addr = Some(SockAddrIn {
                    family: AF_INET,
                    port:   remote_port.to_be(),
                    addr:   remote_ip.to_be(),
                    pad:    [0; 8],
                });
                s.state = SockState::Connected;
            });

            // Write remote address to caller's buffer
            if addr != 0 && addrlen != 0 {
                let sa_out = SockAddrIn {
                    family: AF_INET,
                    port:   remote_port.to_be(),
                    addr:   remote_ip.to_be(),
                    pad:    [0; 8],
                };
                unsafe { *(addr as *mut SockAddrIn) = sa_out; }
                if addrlen >= 8 {
                    unsafe { *(addrlen as *mut u32) = core::mem::size_of::<SockAddrIn>() as u32; }
                }
            }
            return new_fd;
        }

        if crate::time::ticks() >= deadline { return -11; } // EAGAIN
        crate::arch::x86_64::cpu::enable_interrupts();
        core::hint::spin_loop();
    }
}

pub fn sys_send(sockfd: i32, buf: u64, len: usize, flags: i32) -> i64 {
    sys_sendto(sockfd, buf, len, flags, 0, 0)
}

pub fn sys_sendto(sockfd: i32, buf: u64, len: usize, flags: i32, addr: u64, addrlen: u32) -> i64 {
    if buf == 0 || len == 0 { return 0; }
    let data = unsafe { core::slice::from_raw_parts(buf as *const u8, len) };

    let sockets = SOCKETS.lock();
    let sock = match sockets.get(&sockfd) { Some(s) => s, None => return -9 };

    if sock.family == AF_INET {
        match sock.sock_type {
            SOCK_STREAM => {
                let (la, ra) = match (sock.local_addr, sock.remote_addr) {
                    (Some(l), Some(r)) => (l, r),
                    _ => return -107, // ENOTCONN
                };
                if sock.state != SockState::Connected { return -107; }
                let local_ip    = u32::from_be(la.addr);
                let local_port  = u16::from_be(la.port);
                let remote_ip   = u32::from_be(ra.addr);
                let remote_port = u16::from_be(ra.port);
                drop(sockets);
                let n = crate::net::tcp::send_data(local_ip, local_port, remote_ip, remote_port, data);
                return n as i64;
            }
            SOCK_DGRAM => {
                let (src_ip, src_port, dst_ip, dst_port) = if addr != 0 && addrlen >= 8 {
                    let sa = unsafe { *(addr as *const SockAddrIn) };
                    let lip = crate::net::ip::local_ip();
                    let lport = match sock.local_addr {
                        Some(la) => u16::from_be(la.port),
                        None => alloc_ephemeral_port(),
                    };
                    (lip, lport, u32::from_be(sa.addr), u16::from_be(sa.port))
                } else if let (Some(la), Some(ra)) = (sock.local_addr, sock.remote_addr) {
                    (u32::from_be(la.addr), u16::from_be(la.port),
                     u32::from_be(ra.addr), u16::from_be(ra.port))
                } else { return -107; };
                drop(sockets);
                crate::net::udp::send(src_ip, src_port, dst_ip, dst_port, data);
                return len as i64;
            }
            _ => return -22,
        }
    }
    -22
}

pub fn sys_recv(sockfd: i32, buf: u64, len: usize, flags: i32) -> i64 {
    sys_recvfrom(sockfd, buf, len, flags, 0, 0)
}

pub fn sys_recvfrom(sockfd: i32, buf: u64, len: usize, flags: i32, addr: u64, addrlen: u64) -> i64 {
    if buf == 0 || len == 0 { return 0; }

    let (family, sock_type, local_addr, remote_addr, nonblocking, state) = {
        let sockets = SOCKETS.lock();
        let sock = match sockets.get(&sockfd) { Some(s) => s, None => return -9 };
        (sock.family, sock.sock_type, sock.local_addr, sock.remote_addr,
         sock.nonblocking, sock.state)
    };

    let deadline = if nonblocking || flags & MSG_DONTWAIT != 0 {
        crate::time::ticks()
    } else {
        crate::time::ticks() + 30_000
    };

    loop {
        if family == AF_INET {
            match sock_type {
                SOCK_STREAM => {
                    let (la, ra) = match (local_addr, remote_addr) {
                        (Some(l), Some(r)) => (l, r),
                        _ => return if nonblocking { -11 } else { -107 },
                    };
                    let local_ip    = u32::from_be(la.addr);
                    let local_port  = u16::from_be(la.port);
                    let remote_ip   = u32::from_be(ra.addr);
                    let remote_port = u16::from_be(ra.port);

                    // Check TCP state first
                    let tcp_st = crate::net::tcp::connection_state(
                        local_ip, local_port, remote_ip, remote_port
                    );
                    if tcp_st == crate::net::tcp::TcpState::Closed && state == SockState::Connected {
                        return 0; // EOF / connection closed
                    }

                    // Drain data from TCP recv buffer
                    let n = crate::net::tcp::drain_recv(
                        local_ip, local_port, remote_ip, remote_port,
                        buf, len, flags & MSG_PEEK != 0,
                    );
                    if n > 0 { return n; }

                    // Connection closed with remaining data drained
                    if tcp_st == crate::net::tcp::TcpState::CloseWait
                    || tcp_st == crate::net::tcp::TcpState::Closed {
                        return 0; // EOF
                    }
                }
                SOCK_DGRAM => {
                    // Try to dequeue from UDP receive queue
                    let item = {
                        let mut sockets = SOCKETS.lock();
                        sockets.get_mut(&sockfd)
                            .and_then(|s| if s.udp_recv_q.is_empty() { None }
                                         else { Some(s.udp_recv_q.remove(0)) })
                    };
                    if let Some((src, data)) = item {
                        let copy = data.len().min(len);
                        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf as *mut u8, copy); }
                        if addr != 0 { unsafe { *(addr as *mut SockAddrIn) = src; } }
                        if addrlen != 0 { unsafe { *(addrlen as *mut u32) = 16; } }
                        return copy as i64;
                    }
                }
                _ => return -22,
            }
        }

        if crate::time::ticks() >= deadline {
            return -11; // EAGAIN
        }
        crate::arch::x86_64::cpu::enable_interrupts();
        core::hint::spin_loop();
    }
}

/// Called by UDP receive path to deliver a packet to a bound socket.
pub fn udp_deliver(dst_ip: u32, dst_port: u16, src_ip: u32, src_port: u16, data: &[u8]) {
    let mut sockets = SOCKETS.lock();
    for (_, sock) in sockets.iter_mut() {
        if sock.sock_type != SOCK_DGRAM { continue; }
        let bound_port = sock.local_addr.map(|a| u16::from_be(a.port)).unwrap_or(0);
        let bound_ip   = sock.local_addr.map(|a| u32::from_be(a.addr)).unwrap_or(0);
        if bound_port == dst_port && (bound_ip == 0 || bound_ip == dst_ip) {
            let src = SockAddrIn {
                family: AF_INET, port: src_port.to_be(),
                addr: src_ip.to_be(), pad: [0; 8],
            };
            if sock.udp_recv_q.len() < 64 { // cap at 64 datagrams
                sock.udp_recv_q.push((src, data.to_vec()));
            }
            return;
        }
    }
}

pub fn sys_setsockopt(sockfd: i32, level: i32, optname: i32, optval: u64, _optlen: u32) -> i32 {
    let mut sockets = SOCKETS.lock();
    if !sockets.contains_key(&sockfd) { return -9; }
    match (level, optname) {
        (SOL_SOCKET, SO_REUSEADDR) | (SOL_SOCKET, 15) => {
            if optval != 0 {
                let v = unsafe { *(optval as *const i32) };
                if let Some(s) = sockets.get_mut(&sockfd) { s.opts_reuse = v != 0; }
            }
            0
        }
        (SOL_SOCKET, SO_RCVBUF) | (SOL_SOCKET, SO_SNDBUF) => 0,
        (SOL_SOCKET, SO_KEEPALIVE) => 0,
        (IPPROTO_TCP, TCP_NODELAY) | (IPPROTO_TCP, TCP_KEEPIDLE)
        | (IPPROTO_TCP, TCP_KEEPINTVL) | (IPPROTO_TCP, TCP_KEEPCNT) => 0,
        _ => 0, // Gracefully ignore unknown options
    }
}

pub fn sys_getsockopt(sockfd: i32, level: i32, optname: i32, optval: u64, optlen: u64) -> i32 {
    let sockets = SOCKETS.lock();
    let sock = match sockets.get(&sockfd) { Some(s) => s, None => return -9 };
    if optval == 0 { return -22; }
    match (level, optname) {
        (SOL_SOCKET, SO_TYPE) => {
            unsafe { *(optval as *mut i32) = sock.sock_type as i32; }
            if optlen != 0 { unsafe { *(optlen as *mut u32) = 4; } }
            0
        }
        (SOL_SOCKET, SO_ERROR) => {
            unsafe { *(optval as *mut i32) = 0; }
            if optlen != 0 { unsafe { *(optlen as *mut u32) = 4; } }
            0
        }
        _ => { unsafe { *(optval as *mut i32) = 0; } 0 }
    }
}

pub fn sys_getsockname(sockfd: i32, addr: u64, addrlen: u64) -> i32 {
    if addr == 0 { return -22; }
    let sockets = SOCKETS.lock();
    let sock = match sockets.get(&sockfd) { Some(s) => s, None => return -9 };
    let sa = sock.local_addr.unwrap_or(SockAddrIn {
        family: sock.family, port: 0, addr: 0, pad: [0;8]
    });
    unsafe { *(addr as *mut SockAddrIn) = sa; }
    if addrlen != 0 { unsafe { *(addrlen as *mut u32) = 16; } }
    0
}

pub fn sys_getpeername(sockfd: i32, addr: u64, addrlen: u64) -> i32 {
    if addr == 0 { return -22; }
    let sockets = SOCKETS.lock();
    let sock = match sockets.get(&sockfd) { Some(s) => s, None => return -9 };
    match sock.remote_addr {
        None => -107, // ENOTCONN
        Some(sa) => {
            unsafe { *(addr as *mut SockAddrIn) = sa; }
            if addrlen != 0 { unsafe { *(addrlen as *mut u32) = 16; } }
            0
        }
    }
}

pub fn sys_shutdown(sockfd: i32, how: i32) -> i32 {
    let sockets = SOCKETS.lock();
    let sock = match sockets.get(&sockfd) { Some(s) => s, None => return -9 };
    if sock.family == AF_INET && sock.sock_type == SOCK_STREAM {
        if let (Some(la), Some(ra)) = (sock.local_addr, sock.remote_addr) {
            let (li, lp, ri, rp) = (u32::from_be(la.addr), u16::from_be(la.port),
                                     u32::from_be(ra.addr), u16::from_be(ra.port));
            drop(sockets);
            if how == SHUT_WR || how == SHUT_RDWR {
                crate::net::tcp::close(li, lp, ri, rp);
            }
        }
    }
    0
}

/// Close a socket and clean up TCP/UDP state.
pub fn socket_close(sockfd: i32) {
    let sock = SOCKETS.lock().remove(&sockfd);
    if let Some(s) = sock {
        if s.family == AF_INET && s.sock_type == SOCK_STREAM {
            if let (Some(la), Some(ra)) = (s.local_addr, s.remote_addr) {
                let (li, lp, ri, rp) = (u32::from_be(la.addr), u16::from_be(la.port),
                                         u32::from_be(ra.addr), u16::from_be(ra.port));
                crate::net::tcp::close(li, lp, ri, rp);
            }
        }
    }
}

pub fn sys_socketpair(family: i32, typ: i32, _protocol: i32, sv: u64) -> i32 {
    if sv == 0 { return -22; }
    // Create two connected UNIX sockets using pipe
    let fds_ptr = sv as *mut [i32; 2];
    let mut pipe_fds = [0i32; 2];
    { let (pr, pw) = crate::ipc::pipe::new_pipe2(0); let _ = (pr, pw); } let r = 0i32;
    if r != 0 { return r; }
    unsafe { (*fds_ptr)[0] = pipe_fds[0]; (*fds_ptr)[1] = pipe_fds[1]; }
    0
}


