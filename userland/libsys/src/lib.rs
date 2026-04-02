//! libsys — minimal syscall interface for Qunix userland.
//! All syscall numbers match Linux x86_64 for binary compatibility.

#![no_std]
#![feature(alloc_error_handler)]

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicBool, Ordering};

struct SysAlloc;

#[global_allocator]
static GLOBAL_ALLOC: SysAlloc = SysAlloc;
static ALLOC_LOCK: AtomicBool = AtomicBool::new(false);
static mut HEAP_CUR: u64 = 0;

#[inline(always)]
fn alloc_lock() {
    while ALLOC_LOCK
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
}

#[inline(always)]
fn alloc_unlock() {
    ALLOC_LOCK.store(false, Ordering::Release);
}

unsafe impl GlobalAlloc for SysAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1) as u64;
        let align = layout.align() as u64;

        alloc_lock();

        if HEAP_CUR == 0 {
            let cur = syscall::syscall1(SYS_BRK, 0);
            if cur <= 0 {
                alloc_unlock();
                return core::ptr::null_mut();
            }
            HEAP_CUR = cur as u64;
        }

        let aligned = (HEAP_CUR + align - 1) & !(align - 1);
        let new_end = aligned.saturating_add(size);
        let brk_res = syscall::syscall1(SYS_BRK, new_end);

        if brk_res < 0 || (brk_res as u64) < new_end {
            alloc_unlock();
            return core::ptr::null_mut();
        }

        HEAP_CUR = new_end;
        alloc_unlock();
        aligned as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[alloc_error_handler]
fn alloc_error(_layout: Layout) -> ! {
    unsafe { syscall::syscall1(SYS_EXIT, 1); }
    loop {}
}

pub mod syscall {
    #[inline(always)]
    pub unsafe fn syscall0(nr: u64) -> i64 {
        let r: i64; core::arch::asm!("syscall", in("rax") nr, lateout("rax") r, out("rcx") _, out("r11") _); r
    }
    #[inline(always)]
    pub unsafe fn syscall1(nr: u64, a1: u64) -> i64 {
        let r: i64; core::arch::asm!("syscall", in("rax") nr, in("rdi") a1, lateout("rax") r, out("rcx") _, out("r11") _); r
    }
    #[inline(always)]
    pub unsafe fn syscall2(nr: u64, a1: u64, a2: u64) -> i64 {
        let r: i64; core::arch::asm!("syscall", in("rax") nr, in("rdi") a1, in("rsi") a2, lateout("rax") r, out("rcx") _, out("r11") _); r
    }
    #[inline(always)]
    pub unsafe fn syscall3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
        let r: i64; core::arch::asm!("syscall", in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3, lateout("rax") r, out("rcx") _, out("r11") _); r
    }
    #[inline(always)]
    pub unsafe fn syscall4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
        let r: i64; core::arch::asm!("syscall", in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4, lateout("rax") r, out("rcx") _, out("r11") _); r
    }
    #[inline(always)]
    pub unsafe fn syscall5(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
        let r: i64; core::arch::asm!("syscall", in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4, in("r8") a5, lateout("rax") r, out("rcx") _, out("r11") _); r
    }
    #[inline(always)]
    pub unsafe fn syscall6(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64, a6: u64) -> i64 {
        let r: i64; core::arch::asm!("syscall", in("rax") nr, in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4, in("r8") a5, in("r9") a6, lateout("rax") r, out("rcx") _, out("r11") _); r
    }
}

// ── Syscall numbers ───────────────────────────────────────────────────────
pub const SYS_READ:      u64 = 0;   pub const SYS_WRITE:    u64 = 1;
pub const SYS_OPEN:      u64 = 2;   pub const SYS_CLOSE:    u64 = 3;
pub const SYS_STAT:      u64 = 4;   pub const SYS_FSTAT:    u64 = 5;
pub const SYS_LSTAT:     u64 = 6;   pub const SYS_POLL:     u64 = 7;
pub const SYS_LSEEK:     u64 = 8;   pub const SYS_MMAP:     u64 = 9;
pub const SYS_MPROTECT:  u64 = 10;  pub const SYS_MUNMAP:   u64 = 11;
pub const SYS_BRK:       u64 = 12;  pub const SYS_RT_SIGACTION: u64 = 13;
pub const SYS_RT_SIGPROCMASK: u64 = 14; pub const SYS_IOCTL: u64 = 16;
pub const SYS_PREAD64:   u64 = 17;  pub const SYS_PWRITE64: u64 = 18;
pub const SYS_READV:     u64 = 19;  pub const SYS_WRITEV:   u64 = 20;
pub const SYS_ACCESS:    u64 = 21;  pub const SYS_PIPE:     u64 = 22;
pub const SYS_SELECT:    u64 = 23;  pub const SYS_SCHED_YIELD: u64 = 24;
pub const SYS_DUP:       u64 = 32;  pub const SYS_DUP2:     u64 = 33;
pub const SYS_NANOSLEEP: u64 = 35;  pub const SYS_ALARM:    u64 = 37;
pub const SYS_GETPID:    u64 = 39;  pub const SYS_SENDFILE: u64 = 40;
pub const SYS_SOCKET:    u64 = 41;  pub const SYS_CONNECT:  u64 = 42;
pub const SYS_ACCEPT:    u64 = 43;  pub const SYS_SENDTO:   u64 = 44;
pub const SYS_RECVFROM:  u64 = 45;  pub const SYS_SHUTDOWN: u64 = 48;
pub const SYS_BIND:      u64 = 49;  pub const SYS_LISTEN:   u64 = 50;
pub const SYS_SETSOCKOPT:u64 = 54;  pub const SYS_GETSOCKOPT:u64 = 55;
pub const SYS_CLONE:     u64 = 56;  pub const SYS_FORK:     u64 = 57;
pub const SYS_VFORK:     u64 = 58;  pub const SYS_EXECVE:   u64 = 59;
pub const SYS_EXIT:      u64 = 60;  pub const SYS_WAIT4:    u64 = 61;
pub const SYS_KILL:      u64 = 62;  pub const SYS_UNAME:    u64 = 63;
pub const SYS_FCNTL:     u64 = 72;  pub const SYS_FSYNC:    u64 = 74;
pub const SYS_TRUNCATE:  u64 = 76;  pub const SYS_FTRUNCATE:u64 = 77;
pub const SYS_GETDENTS64:u64 = 78;  pub const SYS_GETCWD:   u64 = 79;
pub const SYS_CHDIR:     u64 = 80;  pub const SYS_RENAME:   u64 = 82;
pub const SYS_MKDIR:     u64 = 83;  pub const SYS_RMDIR:    u64 = 84;
pub const SYS_CREAT:     u64 = 85;  pub const SYS_LINK:     u64 = 86;
pub const SYS_UNLINK:    u64 = 87;  pub const SYS_SYMLINK:  u64 = 88;
pub const SYS_READLINK:  u64 = 89;  pub const SYS_CHMOD:    u64 = 90;
pub const SYS_CHOWN:     u64 = 92;  pub const SYS_UMASK:    u64 = 95;
pub const SYS_GETTIMEOFDAY: u64 = 96; pub const SYS_GETRLIMIT: u64 = 97;
pub const SYS_GETUID:    u64 = 102; pub const SYS_GETGID:   u64 = 104;
pub const SYS_SETUID:    u64 = 105; pub const SYS_SETGID:   u64 = 106;
pub const SYS_GETEUID:   u64 = 107; pub const SYS_GETEGID:  u64 = 108;
pub const SYS_GETPPID:   u64 = 110; pub const SYS_GETPGRP:  u64 = 111;
pub const SYS_SETSID:    u64 = 112; pub const SYS_SETPGID:  u64 = 109;
pub const SYS_STATFS:    u64 = 137; pub const SYS_FSTATFS:  u64 = 138;
pub const SYS_PRCTL:     u64 = 157; pub const SYS_ARCH_PRCTL: u64 = 158;
pub const SYS_SETRLIMIT: u64 = 160; pub const SYS_SYNC:     u64 = 162;
pub const SYS_REBOOT:    u64 = 169; pub const SYS_SYSINFO:  u64 = 99;
pub const SYS_TIMES:     u64 = 100; pub const SYS_GETPRIORITY: u64 = 140;
pub const SYS_SETPRIORITY: u64 = 141; pub const SYS_SCHED_YIELD2: u64 = 24;
pub const SYS_GETDENTS:  u64 = 78;  // alias
pub const SYS_DUP3:      u64 = 292; pub const SYS_PIPE2:    u64 = 293;
pub const SYS_ACCEPT4:   u64 = 288; pub const SYS_RECVMMSG: u64 = 299;
pub const SYS_SENDMMSG:  u64 = 307; pub const SYS_GETRANDOM:u64 = 318;
pub const SYS_MEMFD_CREATE: u64 = 319; pub const SYS_OPENAT: u64 = 257;
pub const SYS_MKDIRAT:   u64 = 258; pub const SYS_UNLINKAT: u64 = 263;
pub const SYS_RENAMEAT:  u64 = 264; pub const SYS_READLINKAT: u64 = 267;
pub const SYS_FSTATAT:   u64 = 262; pub const SYS_FACCESSAT: u64 = 269;
pub const SYS_PSELECT6:  u64 = 270; pub const SYS_PPOLL:    u64 = 271;
pub const SYS_SET_ROBUST_LIST: u64 = 273; pub const SYS_EPOLL_CREATE1: u64 = 291;
pub const SYS_EPOLL_CTL: u64 = 233; pub const SYS_EPOLL_WAIT: u64 = 232;
pub const SYS_EPOLL_PWAIT: u64 = 281; pub const SYS_SIGNALFD4: u64 = 289;
pub const SYS_TIMERFD_CREATE: u64 = 283; pub const SYS_EVENTFD2: u64 = 290;
pub const SYS_FALLOCATE: u64 = 285; pub const SYS_PRLIMIT64: u64 = 302;
pub const SYS_FUTEX:     u64 = 202; pub const SYS_SET_TID_ADDRESS: u64 = 218;
pub const SYS_CLOCK_GETTIME: u64 = 228; pub const SYS_CLOCK_NANOSLEEP: u64 = 230;
pub const SYS_CLOCK_GETRES: u64 = 229; pub const SYS_EXIT_GROUP: u64 = 231;
pub const SYS_TGKILL:    u64 = 234; pub const SYS_MADVISE:  u64 = 28;
pub const SYS_WAITPID:   u64 = 61;  // alias for wait4

// ── Standard file descriptors ─────────────────────────────────────────────
pub const STDIN:  i32 = 0;
pub const STDOUT: i32 = 1;
pub const STDERR: i32 = 2;

// ── Open flags ────────────────────────────────────────────────────────────
pub const O_RDONLY:   i32 = 0;       pub const O_WRONLY: i32 = 1;
pub const O_RDWR:     i32 = 2;       pub const O_CREAT:  i32 = 0o100;
pub const O_EXCL:     i32 = 0o200;   pub const O_TRUNC:  i32 = 0o1000;
pub const O_APPEND:   i32 = 0o2000;  pub const O_NONBLOCK: i32 = 0o4000;
pub const O_CLOEXEC:  i32 = 0o2000000;
pub const O_DIRECTORY:i32 = 0o200000;

// ── Signal numbers ────────────────────────────────────────────────────────
pub const SIGHUP:    i32 = 1;   pub const SIGINT:  i32 = 2;
pub const SIGQUIT:   i32 = 3;   pub const SIGILL:  i32 = 4;
pub const SIGABRT:   i32 = 6;   pub const SIGFPE:  i32 = 8;
pub const SIGKILL:   i32 = 9;   pub const SIGSEGV: i32 = 11;
pub const SIGPIPE:   i32 = 13;  pub const SIGALRM: i32 = 14;
pub const SIGTERM:   i32 = 15;  pub const SIGCHLD: i32 = 17;
pub const SIGCONT:   i32 = 18;  pub const SIGSTOP: i32 = 19;
pub const SIGTSTP:   i32 = 20;  pub const SIGWINCH:i32 = 28;

// ── Stat structure ────────────────────────────────────────────────────────
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct Stat {
    pub st_dev:     u64, pub st_ino:     u64, pub st_nlink:   u64,
    pub st_mode:    u32, pub st_uid:     u32, pub st_gid:     u32,
    pub _pad0:      u32, pub st_rdev:    u64, pub st_size:    i64,
    pub st_blksize: i64, pub st_blocks:  i64,
    pub st_atime:   i64, pub st_atime_ns:i64,
    pub st_mtime:   i64, pub st_mtime_ns:i64,
    pub st_ctime:   i64, pub st_ctime_ns:i64,
    pub _unused:    [i64; 3],
}

pub const S_IFMT:  u32 = 0xF000;
pub const S_IFREG: u32 = 0x8000; pub const S_IFDIR: u32 = 0x4000;
pub const S_IFCHR: u32 = 0x2000; pub const S_IFLNK: u32 = 0xA000;
pub const S_IFIFO: u32 = 0x1000; pub const S_IFBLK: u32 = 0x6000;

// ── Dirent ────────────────────────────────────────────────────────────────
#[repr(C)]
pub struct Dirent64 {
    pub d_ino:    u64,
    pub d_off:    i64,
    pub d_reclen: u16,
    pub d_type:   u8,
    // name follows as NUL-terminated
}
pub const DT_UNKNOWN: u8 = 0; pub const DT_FIFO: u8 = 1;
pub const DT_CHR: u8 = 2;     pub const DT_DIR: u8 = 4;
pub const DT_BLK: u8 = 6;     pub const DT_REG: u8 = 8;
pub const DT_LNK: u8 = 10;    pub const DT_SOCK: u8 = 12;

// ── Syscall wrappers ──────────────────────────────────────────────────────

pub fn write(fd: i32, s: &[u8]) -> i64 {
    unsafe { syscall::syscall3(SYS_WRITE, fd as u64, s.as_ptr() as u64, s.len() as u64) }
}
pub fn read(fd: i32, buf: &mut [u8]) -> i64 {
    unsafe { syscall::syscall3(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) }
}
pub fn open(path: &[u8], flags: i32, mode: u32) -> i64 {
    unsafe { syscall::syscall3(SYS_OPEN, path.as_ptr() as u64, flags as u64, mode as u64) }
}
pub fn openat(dirfd: i32, path: &[u8], flags: i32, mode: u32) -> i64 {
    unsafe { syscall::syscall4(SYS_OPENAT, dirfd as u64, path.as_ptr() as u64, flags as u64, mode as u64) }
}
pub fn close(fd: i32) -> i64 { unsafe { syscall::syscall1(SYS_CLOSE, fd as u64) } }
pub fn exit(code: i32) -> ! {
    unsafe { syscall::syscall1(SYS_EXIT_GROUP, code as u64); core::hint::unreachable_unchecked() }
}
pub fn fork() -> i64 { unsafe { syscall::syscall0(SYS_FORK) } }
pub fn execve(path: &[u8], argv: &[*const u8], envp: &[*const u8]) -> i64 {
    unsafe { syscall::syscall3(SYS_EXECVE, path.as_ptr() as u64, argv.as_ptr() as u64, envp.as_ptr() as u64) }
}
pub fn waitpid(pid: i32, status: *mut i32, options: i32) -> i64 {
    unsafe { syscall::syscall4(SYS_WAIT4, pid as u64, status as u64, options as u64, 0) }
}
pub fn wait4(pid: i32, status: *mut i32, options: i32, rusage: u64) -> i64 {
    unsafe { syscall::syscall4(SYS_WAIT4, pid as u64, status as u64, options as u64, rusage) }
}
pub fn getpid()  -> i64 { unsafe { syscall::syscall0(SYS_GETPID) } }
pub fn getppid() -> i64 { unsafe { syscall::syscall0(SYS_GETPPID) } }
pub fn gettid()  -> i64 { unsafe { syscall::syscall0(186) } }
pub fn getuid()  -> i64 { unsafe { syscall::syscall0(SYS_GETUID) } }
pub fn getgid()  -> i64 { unsafe { syscall::syscall0(SYS_GETGID) } }
pub fn geteuid() -> i64 { unsafe { syscall::syscall0(SYS_GETEUID) } }
pub fn getegid() -> i64 { unsafe { syscall::syscall0(SYS_GETEGID) } }
pub fn setsid()  -> i64 { unsafe { syscall::syscall0(SYS_SETSID) } }
pub fn setpgid(pid: i32, pgid: i32) -> i64 {
    unsafe { syscall::syscall2(SYS_SETPGID, pid as u64, pgid as u64) }
}
pub fn kill(pid: i32, sig: i32) -> i64 {
    unsafe { syscall::syscall2(SYS_KILL, pid as u64, sig as u64) }
}
pub fn tgkill(tgid: i32, tid: i32, sig: i32) -> i64 {
    unsafe { syscall::syscall3(SYS_TGKILL, tgid as u64, tid as u64, sig as u64) }
}

pub fn chdir(path: &[u8]) -> i64 { unsafe { syscall::syscall1(SYS_CHDIR, path.as_ptr() as u64) } }
pub fn fchdir(fd: i32) -> i64 { unsafe { syscall::syscall1(81, fd as u64) } }
pub fn getcwd(buf: &mut [u8]) -> i64 { unsafe { syscall::syscall2(SYS_GETCWD, buf.as_mut_ptr() as u64, buf.len() as u64) } }
pub fn mkdir(path: &[u8], mode: u32) -> i64 { unsafe { syscall::syscall2(SYS_MKDIR, path.as_ptr() as u64, mode as u64) } }
pub fn rmdir(path: &[u8]) -> i64 { unsafe { syscall::syscall1(SYS_RMDIR, path.as_ptr() as u64) } }
pub fn unlink(path: &[u8]) -> i64 { unsafe { syscall::syscall1(SYS_UNLINK, path.as_ptr() as u64) } }
pub fn rename(old: &[u8], new: &[u8]) -> i64 { unsafe { syscall::syscall2(SYS_RENAME, old.as_ptr() as u64, new.as_ptr() as u64) } }
pub fn symlink(target: &[u8], link: &[u8]) -> i64 { unsafe { syscall::syscall2(SYS_SYMLINK, target.as_ptr() as u64, link.as_ptr() as u64) } }
pub fn readlink(path: &[u8], buf: &mut [u8]) -> i64 { unsafe { syscall::syscall3(SYS_READLINK, path.as_ptr() as u64, buf.as_mut_ptr() as u64, buf.len() as u64) } }
pub fn chmod(path: &[u8], mode: u32) -> i64 { unsafe { syscall::syscall2(SYS_CHMOD, path.as_ptr() as u64, mode as u64) } }
pub fn chown(path: &[u8], uid: u32, gid: u32) -> i64 { unsafe { syscall::syscall3(SYS_CHOWN, path.as_ptr() as u64, uid as u64, gid as u64) } }
pub fn truncate(path: &[u8], len: i64) -> i64 { unsafe { syscall::syscall2(SYS_TRUNCATE, path.as_ptr() as u64, len as u64) } }
pub fn ftruncate(fd: i32, len: i64) -> i64 { unsafe { syscall::syscall2(SYS_FTRUNCATE, fd as u64, len as u64) } }
pub fn fsync(fd: i32) -> i64 { unsafe { syscall::syscall1(SYS_FSYNC, fd as u64) } }
pub fn sync() -> i64 { unsafe { syscall::syscall0(SYS_SYNC) } }

pub fn lseek(fd: i32, offset: i64, whence: i32) -> i64 { unsafe { syscall::syscall3(SYS_LSEEK, fd as u64, offset as u64, whence as u64) } }
pub fn pread(fd: i32, buf: &mut [u8], offset: i64) -> i64 { unsafe { syscall::syscall4(SYS_PREAD64, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, offset as u64) } }
pub fn pwrite(fd: i32, buf: &[u8], offset: i64) -> i64 { unsafe { syscall::syscall4(SYS_PWRITE64, fd as u64, buf.as_ptr() as u64, buf.len() as u64, offset as u64) } }

pub fn stat(path: &[u8], st: &mut Stat) -> i64 { unsafe { syscall::syscall2(SYS_STAT, path.as_ptr() as u64, st as *mut _ as u64) } }
pub fn lstat(path: &[u8], st: &mut Stat) -> i64 { unsafe { syscall::syscall2(SYS_LSTAT, path.as_ptr() as u64, st as *mut _ as u64) } }
pub fn fstat(fd: i32, st: &mut Stat) -> i64 { unsafe { syscall::syscall2(SYS_FSTAT, fd as u64, st as *mut _ as u64) } }
pub fn access(path: &[u8], mode: i32) -> i64 { unsafe { syscall::syscall2(SYS_ACCESS, path.as_ptr() as u64, mode as u64) } }

pub fn dup(fd: i32) -> i64 { unsafe { syscall::syscall1(SYS_DUP, fd as u64) } }
pub fn dup2(oldfd: i32, newfd: i32) -> i64 { unsafe { syscall::syscall2(SYS_DUP2, oldfd as u64, newfd as u64) } }
pub fn dup3(oldfd: i32, newfd: i32, flags: i32) -> i64 { unsafe { syscall::syscall3(SYS_DUP3, oldfd as u64, newfd as u64, flags as u64) } }

pub fn pipe(fds: &mut [i32; 2]) -> i64 { unsafe { syscall::syscall1(SYS_PIPE, fds.as_mut_ptr() as u64) } }
pub fn pipe2(fds: &mut [i32; 2], flags: i32) -> i64 { unsafe { syscall::syscall2(SYS_PIPE2, fds.as_mut_ptr() as u64, flags as u64) } }

pub fn fcntl(fd: i32, cmd: i32, arg: u64) -> i64 { unsafe { syscall::syscall3(SYS_FCNTL, fd as u64, cmd as u64, arg) } }
pub fn ioctl(fd: i32, req: u64, arg: u64) -> i64 { unsafe { syscall::syscall3(SYS_IOCTL, fd as u64, req, arg) } }

pub fn getdents64(fd: i32, buf: &mut [u8]) -> i64 { unsafe { syscall::syscall3(SYS_GETDENTS64, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) } }

pub fn mmap(addr: u64, len: usize, prot: i32, flags: i32, fd: i32, offset: i64) -> i64 {
    unsafe { syscall::syscall6(SYS_MMAP, addr, len as u64, prot as u64, flags as u64, fd as u64, offset as u64) }
}
pub fn munmap(addr: u64, len: usize) -> i64 { unsafe { syscall::syscall2(SYS_MUNMAP, addr, len as u64) } }
pub fn mprotect(addr: u64, len: usize, prot: i32) -> i64 { unsafe { syscall::syscall3(SYS_MPROTECT, addr, len as u64, prot as u64) } }
pub fn madvise(addr: u64, len: usize, advice: i32) -> i64 { unsafe { syscall::syscall3(SYS_MADVISE, addr, len as u64, advice as u64) } }
pub fn brk(addr: u64) -> i64 { unsafe { syscall::syscall1(SYS_BRK, addr) } }

pub fn nanosleep_ms(ms: u64) -> i64 {
    let ts: [u64; 2] = [ms / 1000, (ms % 1000) * 1_000_000];
    unsafe { syscall::syscall2(SYS_NANOSLEEP, ts.as_ptr() as u64, 0) }
}
pub fn nanosleep(secs: i64, nsecs: i64) -> i64 {
    let ts: [i64; 2] = [secs, nsecs];
    unsafe { syscall::syscall2(SYS_NANOSLEEP, ts.as_ptr() as u64, 0) }
}
pub fn clock_gettime(clkid: i32, ts: &mut [i64; 2]) -> i64 { unsafe { syscall::syscall2(SYS_CLOCK_GETTIME, clkid as u64, ts.as_mut_ptr() as u64) } }

pub fn set_tid_address(tidptr: u64) -> i64 { unsafe { syscall::syscall1(SYS_SET_TID_ADDRESS, tidptr) } }
pub fn set_robust_list(head: u64, len: usize) -> i64 { unsafe { syscall::syscall2(SYS_SET_ROBUST_LIST, head, len as u64) } }

pub fn rt_sigaction(sig: i32, act: u64, oact: u64, sigsetsize: usize) -> i64 {
    unsafe { syscall::syscall4(SYS_RT_SIGACTION, sig as u64, act, oact, sigsetsize as u64) }
}
pub fn rt_sigprocmask(how: i32, set: u64, oset: u64, sigsetsize: usize) -> i64 {
    unsafe { syscall::syscall4(SYS_RT_SIGPROCMASK, how as u64, set, oset, sigsetsize as u64) }
}
pub fn arch_prctl(code: i32, addr: u64) -> i64 { unsafe { syscall::syscall2(SYS_ARCH_PRCTL, code as u64, addr) } }
pub fn prctl(op: i32, a: u64, b: u64, c: u64, d: u64) -> i64 { unsafe { syscall::syscall5(SYS_PRCTL, op as u64, a, b, c, d) } }
pub fn futex(uaddr: u64, op: i32, val: u32, timeout: u64, uaddr2: u64, val3: u32) -> i64 {
    unsafe { syscall::syscall6(SYS_FUTEX, uaddr, op as u64, val as u64, timeout, uaddr2, val3 as u64) }
}
pub fn getrandom(buf: &mut [u8], flags: u32) -> i64 { unsafe { syscall::syscall3(SYS_GETRANDOM, buf.as_mut_ptr() as u64, buf.len() as u64, flags as u64) } }

// Sockets
pub fn socket(fam: i32, typ: i32, proto: i32) -> i64 { unsafe { syscall::syscall3(SYS_SOCKET, fam as u64, typ as u64, proto as u64) } }
pub fn bind(fd: i32, addr: *const u8, len: u32) -> i64 { unsafe { syscall::syscall3(SYS_BIND, fd as u64, addr as u64, len as u64) } }
pub fn listen(fd: i32, backlog: i32) -> i64 { unsafe { syscall::syscall2(SYS_LISTEN, fd as u64, backlog as u64) } }
pub fn connect(fd: i32, addr: *const u8, len: u32) -> i64 { unsafe { syscall::syscall3(SYS_CONNECT, fd as u64, addr as u64, len as u64) } }
pub fn accept(fd: i32, addr: u64, len: u64) -> i64 { unsafe { syscall::syscall3(SYS_ACCEPT, fd as u64, addr, len) } }
pub fn sendto(fd: i32, buf: &[u8], flags: i32, addr: u64, alen: u32) -> i64 {
    unsafe { syscall::syscall6(SYS_SENDTO, fd as u64, buf.as_ptr() as u64, buf.len() as u64, flags as u64, addr, alen as u64) }
}
pub fn recvfrom(fd: i32, buf: &mut [u8], flags: i32, addr: u64, alen: u64) -> i64 {
    unsafe { syscall::syscall6(SYS_RECVFROM, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, flags as u64, addr, alen) }
}
pub fn setsockopt(fd: i32, lvl: i32, opt: i32, val: u64, vlen: u32) -> i64 {
    unsafe { syscall::syscall5(SYS_SETSOCKOPT, fd as u64, lvl as u64, opt as u64, val, vlen as u64) }
}

// Epoll
pub fn epoll_create1(flags: i32) -> i64 { unsafe { syscall::syscall1(SYS_EPOLL_CREATE1, flags as u64) } }
pub fn epoll_ctl(epfd: i32, op: i32, fd: i32, event: u64) -> i64 { unsafe { syscall::syscall4(SYS_EPOLL_CTL, epfd as u64, op as u64, fd as u64, event) } }
pub fn epoll_wait(epfd: i32, events: u64, maxevents: i32, timeout: i32) -> i64 { unsafe { syscall::syscall4(SYS_EPOLL_WAIT, epfd as u64, events, maxevents as u64, timeout as u64) } }

// ── String utilities (no_std) ─────────────────────────────────────────────

pub fn cstr_len(s: &[u8]) -> usize {
    s.iter().position(|&b| b == 0).unwrap_or(s.len())
}

pub fn write_str(fd: i32, s: &str) { write(fd, s.as_bytes()); }

pub fn write_int(fd: i32, mut n: i64) {
    if n < 0 { write(fd, b"-"); n = -n; }
    let mut buf = [0u8; 20];
    let mut i = 19usize;
    if n == 0 { write(fd, b"0"); return; }
    while n > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    write(fd, &buf[i..]);
}

pub fn write_uint(fd: i32, mut n: u64) {
    let mut buf = [0u8; 20]; let mut i = 19usize;
    if n == 0 { write(fd, b"0"); return; }
    while n > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    write(fd, &buf[i..]);
}

pub fn num_to_buf(mut n: u64, buf: &mut [u8]) -> &[u8] {
    let mut i = buf.len();
    if n == 0 { if i > 0 { buf[i-1] = b'0'; return &buf[i-1..]; } return b"0"; }
    while n > 0 && i > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; }
    &buf[i..]
}

pub fn int_to_buf(n: i64, buf: &mut [u8]) -> &[u8] {
    if n < 0 {
        let mut v = n.wrapping_neg() as u64;
        let mut i = buf.len();
        if v == 0 {
            if i > 1 {
                buf[i - 1] = b'0';
                buf[i - 2] = b'-';
                return &buf[i - 2..];
            }
            return b"-";
        }
        while v > 0 && i > 0 {
            i -= 1;
            buf[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        if i > 0 {
            i -= 1;
            buf[i] = b'-';
            return &buf[i..];
        }
        return buf;
    }
    num_to_buf(n as u64, buf)
}

pub fn starts_with(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && &s[..prefix.len()] == prefix
}

pub fn ends_with(s: &[u8], suffix: &[u8]) -> bool {
    s.len() >= suffix.len() && &s[s.len()-suffix.len()..] == suffix
}

pub fn str_eq(a: &[u8], b: &[u8]) -> bool { a == b }

pub fn copy_cstr(dst: &mut [u8], src: &[u8]) -> usize {
    let n = src.len().min(dst.len().saturating_sub(1));
    dst[..n].copy_from_slice(&src[..n]);
    if dst.len() > n { dst[n] = 0; }
    n
}

// ── Missing signal constants ───────────────────────────────────────────────
pub const SIGUSR1:   i32 = 10;
pub const SIGUSR2:   i32 = 12;

// ── Missing syscall wrappers ───────────────────────────────────────────────
pub fn umask(mask: u32) -> i64 {
    unsafe { syscall::syscall1(SYS_UMASK, mask as u64) }
}

pub fn isatty(fd: i32) -> i64 {
    let mut ws = [0u16; 4];
    ioctl(fd, 0x5413, ws.as_mut_ptr() as u64)
}





// Additional constants used by qshell
pub const WNOHANG: i32 = 1;

// ── Additional missing helpers ──────────────────────────────────────────────
pub const O_NOCTTY: i32 = 0o400;
pub const O_DSYNC:  i32 = 0o10000;
pub const O_ASYNC:  i32 = 0o20000;
pub const O_DIRECT: i32 = 0o40000;
pub const O_LARGEFILE: i32 = 0o100000;
pub const O_NOFOLLOW:  i32 = 0o400000;
pub const O_NOATIME:   i32 = 0o1000000;
pub const O_PATH:      i32 = 0o10000000;

// Extra file type / syscall constants used by some userland tools.
pub const S_IFSOCK: u32 = 0xC000;

// Extra syscall numbers
pub const SYS_MKNOD:    u64 = 133;
pub const SYS_LCHOWN:   u64 = 94;
pub const SYS_MOUNT:    u64 = 165;
pub const SYS_UMOUNT2:  u64 = 166;
pub const SYS_FDATASYNC: u64 = 75;
pub const SYS_SETHOSTNAME: u64 = 170;
pub const SYS_WAITID: u64 = 247;
pub const SYS_UTIMENSAT: u64 = 280;

pub const WUNTRACED: i32 = 2;
pub const WCONTINUED: i32 = 8;

pub fn clock_gettime_simple() -> i64 {
    let mut ts = [0i64; 2];
    unsafe { syscall::syscall2(SYS_CLOCK_GETTIME, 1, ts.as_mut_ptr() as u64) };
    ts[0]
}
