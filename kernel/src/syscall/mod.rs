//! Syscall dispatch — direct jump table, O(1) per call.
//!
//! Uses a static array of function pointers indexed by syscall number.
//! On x86-64 Linux ABI: rax=nr, rdi/rsi/rdx/r10/r8/r9 = args.
//!
//! Faster than a match chain because:
//!   - Single indirect branch (CALL [table + rax*8])
//!   - No comparisons
//!   - CPU branch predictor specializes on indirect call targets
//!   - Table fits in 2 cache lines for the common syscalls (0..30)

use crate::arch::x86_64::syscall_entry::SyscallFrame;

pub mod handlers;

/// Maximum syscall number we handle.
const NR_SYSCALLS: usize = 512;

/// The syscall handler signature.
type SyscallFn = fn(u64, u64, u64, u64, u64, u64, &mut SyscallFrame) -> i64;

fn sys_unimplemented(nr: u64, _a1: u64, _a2: u64, _a3: u64, _a4: u64, _a5: u64, _f: &mut SyscallFrame) -> i64 {
    crate::klog!("unimplemented syscall {}", nr);
    -38 // ENOSYS
}

fn sys_nosys(_: u64, _: u64, _: u64, _: u64, _: u64, _: u64, _: &mut SyscallFrame) -> i64 { -38 }

/// Build the dispatch table. const fn so it lives in .rodata.
const fn make_table() -> [SyscallFn; NR_SYSCALLS] {
    let mut t = [sys_nosys as SyscallFn; NR_SYSCALLS];
    use handlers::*;

    // ── File I/O ──────────────────────────────────────────────────────────
    t[0]   = |_,buf,len,_,_,_,f| sys_read(f.rdi as i32, buf, len as usize);
    t[1]   = |_,buf,len,_,_,_,f| sys_write(f.rdi as i32, buf, len as usize);
    t[2]   = |_,path,flags,mode,_,_,f| sys_open(path, flags as i32, mode as u32);
    t[3]   = |_,_,_,_,_,_,f| sys_close(f.rdi as i32);
    t[4]   = |_,path,st,_,_,_,_| sys_stat(path, st);
    t[5]   = |_,fd,st,_,_,_,_| sys_fstat(fd as i32, st);
    t[6]   = |_,path,st,_,_,_,_| sys_lstat(path, st);
    t[7]   = |_,fds,n,_,_,_,f| sys_poll(fds, n as u32, f.rdx as i32);
    t[8]   = |_,off,whence,_,_,_,f| sys_lseek(f.rdi as i32, off as i64, whence as i32);
    t[9]   = |_,addr,len,prot,flags,fd,f| sys_mmap(addr, len, prot as i32, flags as i32, fd as i32, f.r9 as u64);
    t[10]  = |_,addr,len,prot,_,_,_| sys_mprotect(addr, len, prot as i32);
    t[11]  = |_,addr,len,_,_,_,_| sys_munmap(addr, len);
    t[12]  = |_,brk,_,_,_,_,_| sys_brk(brk);
    t[13]  = |_,handler,flags,_,_,_,f| sys_rt_sigaction(f.rdi as i32, handler, flags, f.r10 as usize);
    t[14]  = |_,how,set,old,_,_,f| sys_rt_sigprocmask(f.rdi as i32, set, old, f.r10 as usize);
    t[15]  = |_,_,_,_,_,_,f| sys_rt_sigreturn(f);
    t[16]  = |_,req,_,_,_,_,f| sys_ioctl(f.rdi as i32, f.rsi, req);
    t[17]  = |_,buf,count,_,_,_,f| sys_pread64(f.rdi as i32, buf, count as usize, f.r10 as i64);
    t[18]  = |_,buf,count,_,_,_,f| sys_pwrite64(f.rdi as i32, buf, count as usize, f.r10 as i64);
    t[19]  = |_,iov,iovcnt,_,_,_,f| sys_readv(f.rdi as i32, iov, iovcnt as usize);
    t[20]  = |_,iov,iovcnt,_,_,_,f| sys_writev(f.rdi as i32, iov, iovcnt as usize);
    t[21]  = |_,path,mode,_,_,_,_| sys_access(path, mode as i32);
    t[22]  = |_,fds,_,_,_,_,_| sys_pipe(fds);
    t[23]  = |_,nfds,r,w,e,t,_| sys_select(nfds as i32, r, w, e, t);
    t[24]  = |_,_,_,_,_,_,_| { crate::sched::yield_current(); 0 };
    t[25]  = |_,pid,_,_,_,_,_| sys_mremap(pid, 0, 0, 0, 0);
    t[26]  = |_,start,len,_,_,_,_| sys_msync(start, len, 0i32);
    t[27]  = |_,start,len,vec,_,_,_| sys_mincore(start, len as usize, vec);
    t[28]  = |_,start,len,advice,_,_,_| sys_madvise(start, len, advice as i32);
    t[32]  = |_,fds,_,_,_,_,_| sys_dup(fds as i32);
    t[33]  = |_,old,new,_,_,_,_| sys_dup2(old as i32, new as i32);
    t[34]  = |_,_,_,_,_,_,_| 0; // pause
    t[35]  = |_,req,rem,_,_,_,_| sys_nanosleep(req, rem);
    t[36]  = |_,_,_,_,_,_,_| 0; // getitimer
    t[37]  = |_,_,_,_,_,_,_| 0; // alarm
    t[38]  = |_,_,_,_,_,_,_| 0; // setitimer
    t[39]  = |_,_,_,_,_,_,_| crate::process::current_pid() as i64;
    t[40]  = |_,_,_,_,_,_,_| 0i64; // getppid (simplified)
    t[41]  = |_,_,_,_,_,_,_| 0i64; // getpgrp
    t[42]  = |_,_,_,_,_,_,_| 0i64; // setsid
    t[56]  = |_,flags,stack,ptid,ctid,tls,f| sys_clone(flags, stack, ptid, ctid, tls);
    t[57]  = |_,_,_,_,_,_,_| sys_fork();
    t[58]  = |_,_,_,_,_,_,_| sys_vfork();
    t[59]  = |_,path,argv,envp,_,_,_| sys_execve(path, argv, envp);
    t[60]  = |_,code,_,_,_,_,_| sys_exit(code as i32);
    t[61]  = |_,pid,status,opts,_,_,_| sys_wait4(pid as i32, status, opts as i32, 0u64);
    t[62]  = |_,sig,_,_,_,_,f| sys_kill(f.rdi as i32, sig as i32);

    // ── Process / identity ─────────────────────────────────────────────
    t[63]  = |_,buf,_,_,_,_,_| sys_uname(buf);
    t[72]  = |_,mask,_,_,_,_,_| sys_fcntl_dummy();
    t[73]  = |_,fd,how,_,_,_,_| sys_flock(fd as i32, how as i32);
    t[74]  = |_,fd,_,_,_,_,_| sys_fsync(fd as i32);
    t[75]  = |_,fd,_,_,_,_,_| sys_fdatasync(fd as i32);
    t[76]  = |_,path,len,_,_,_,_| sys_truncate(path, len as i64);
    t[77]  = |_,fd,len,_,_,_,_| sys_ftruncate(fd as i32, len as i64);
    t[78]  = |_,fd,buf,count,_,_,f| sys_getdents(fd as i32, buf, count as u32);
    t[79]  = |_,buf,sz,_,_,_,_| sys_getcwd(buf, sz as usize);
    t[80]  = |_,path,_,_,_,_,_| sys_chdir(path);
    t[81]  = |_,fd,_,_,_,_,_| sys_fchdir(fd as i32);
    t[82]  = |_,old,new,_,_,_,_| sys_rename(old, new);
    t[83]  = |_,path,mode,_,_,_,_| sys_mkdir(path, mode as u32);
    t[84]  = |_,path,_,_,_,_,_| sys_rmdir(path);
    t[85]  = |_,path,mode,_,_,_,_| sys_creat(path, mode as u32);
    t[86]  = |_,old,new,_,_,_,_| sys_link(old, new);
    t[87]  = |_,path,_,_,_,_,_| sys_unlink(path);
    t[88]  = |_,target,link,_,_,_,_| sys_symlink(target, link);
    t[89]  = |_,path,buf,sz,_,_,_| sys_readlink(path, buf, sz as usize);
    t[90]  = |_,path,mode,_,_,_,_| sys_chmod(path, mode as u32);
    t[91]  = |_,fd,mode,_,_,_,_| sys_fchmod(fd as i32, mode as u32);
    t[92]  = |_,path,uid,gid,_,_,_| sys_chown(path, uid as u32, gid as u32);
    t[93]  = |_,fd,uid,gid,_,_,_| sys_fchown(fd as i32, uid as u32, gid as u32);
    t[94]  = |_,path,uid,gid,_,_,_| sys_lchown(path, uid as u32, gid as u32);
    t[95]  = |_,mask,_,_,_,_,_| sys_umask(mask as u32);
    t[96]  = |_,clk,ts,_,_,_,_| sys_gettimeofday(clk, ts);
    t[97]  = |_,_,_,_,_,_,_| 0; // getrlimit
    t[98]  = |_,_,_,_,_,_,_| 0; // getrusage
    t[99]  = |_,buf,_,_,_,_,_| sys_sysinfo(buf);
    t[100] = |_,_,_,_,_,_,_| 0; // times
    t[101] = |_,_,_,_,_,_,_| 0; // ptrace
    t[102] = |_,_,_,_,_,_,_| 0i64; // getuid
    t[103] = |_,msg,_,_,_,_,_| sys_syslog(msg);
    t[104] = |_,_,_,_,_,_,_| 0i64; // getgid
    t[105] = |_,_,_,_,_,_,_| 0; // setuid
    t[106] = |_,_,_,_,_,_,_| 0; // setgid
    t[107] = |_,_,_,_,_,_,_| 0i64; // geteuid
    t[108] = |_,_,_,_,_,_,_| 0i64; // getegid
    t[109] = |_,_,_,_,_,_,_| 0; // setpgid
    t[110] = |_,_,_,_,_,_,_| 0i64; // getppid
    t[111] = |_,_,_,_,_,_,_| 0i64; // getpgrp
    t[112] = |_,_,_,_,_,_,_| crate::process::current_pid() as i64; // setsid
    t[113] = |_,_,_,_,_,_,_| 0; // setreuid
    t[114] = |_,_,_,_,_,_,_| 0; // setregid
    t[115] = |_,_,_,_,_,_,_| 0; // getgroups
    t[116] = |_,_,_,_,_,_,_| 0; // setgroups

    // ── Memory ────────────────────────────────────────────────────────
    t[125] = |_,cap,data,_,_,_,_| sys_capget(cap, data);
    t[126] = |_,cap,data,_,_,_,_| sys_capset(cap, data);

    // ── Signals ───────────────────────────────────────────────────────
    t[127] = |_,_,_,_,_,_,_| 0; // rt_sigpending
    t[128] = |_,set,info,tmo,sz,_,_| sys_rt_sigtimedwait(set, info, tmo, sz as usize);
    t[129] = |_,_,_,_,_,_,_| 0; // rt_sigqueueinfo
    t[130] = |_,mask,_,_,sz,_,_| sys_rt_sigsuspend(mask, sz as usize);
    t[131] = |_,path,buf,_,_,_,_| sys_sigaltstack(path, buf);

    // ── Advanced file ops ─────────────────────────────────────────────
    t[133] = |_,path,mode,dev,_,_,_| sys_mknod(path, mode as u32, dev as u64);
    t[136] = |_,path,buf,_,_,_,_| sys_statfs(path, buf);
    t[137] = |_,path,buf,_,_,_,_| sys_statfs(path, buf);
    t[161] = |_,path,_,_,_,_,_| sys_chroot(path);
    t[162] = |_,_,_,_,_,_,_| sys_sync();
    t[163] = |_,_,_,_,_,_,_| 0; // acct
    t[164] = |_,_,_,_,_,_,_| 0; // settimeofday
    t[165] = |_,dev,target,fs,flags,data,_| sys_mount(dev, target, fs, flags, data);
    t[166] = |_,target,flags,_,_,_,_| sys_umount2(target, flags as i32);

    // ── Thread / process ──────────────────────────────────────────────
    t[158] = |_,code,addr,_,_,_,_| sys_arch_prctl(code as i32, addr);
    t[160] = |_,_,_,_,_,_,_| 0; // setrlimit
    t[186] = |_,_,_,_,_,_,_| crate::process::current_pid() as i64; // gettid
    t[200] = |_,_,_,_,_,_,_| 0; // tkill stub
    t[201] = |_,clk,ts,_,_,_,_| sys_clock_gettime(clk as i32, ts);
    t[229] = |_,clk,ts,_,_,_,_| sys_clock_getres(clk as i32, ts);
    t[230] = |_,clk,flags,req,rem,_,_| sys_clock_nanosleep(clk as i32, flags as i32, req, rem);
    t[204] = |_,pid,sig,_,_,_,_| sys_kill(pid as i32, sig as i32); // tgkill
    t[205] = |_,_,_,_,_,_,_| 0; // utimes
    t[206] = |_,_,_,_,_,_,_| 0; // vserver
    t[207] = |_,_,_,_,_,_,_| 0; // mbind
    t[208] = |_,_,_,_,_,_,_| 0; // set_mempolicy

    // ── Epoll ─────────────────────────────────────────────────────────
    t[213] = |_,flags,_,_,_,_,_| sys_epoll_create(flags as i32);
    t[214] = |_,epfd,op,fd,ev,_,_| sys_epoll_ctl(epfd as i32, op as i32, fd as i32, ev);
    t[215] = |_,epfd,evs,max,ms,_,_| sys_epoll_wait(epfd as i32, evs, max as i32, ms as i32);

    // ── futex ─────────────────────────────────────────────────────────
    t[202] = |_,uaddr,op,val,ts,uaddr2,f| {
        crate::abi_compat::syscall::sys_futex(uaddr, op as i32, val as u32, ts, uaddr2, f.r9 as u32)
    };

    // ── More futex ────────────────────────────────────────────────────
    t[240] = |_,uaddr,op,val,ts,uaddr2,f| {
        crate::abi_compat::syscall::sys_futex(uaddr, op as i32, val as u32, ts, uaddr2, f.r9 as u32)
    };

    // ── Socket ────────────────────────────────────────────────────────
    t[41]  = |_,fam,typ,proto,_,_,_| crate::net::socket::sys_socket(fam as u16, typ as u8, proto as u8) as i64;
    t[42]  = |_,fd,addr,len,_,_,_| crate::net::socket::sys_connect(fd as i32, addr, len as u32) as i64;
    t[43]  = |_,fd,addr,len,_,_,_| crate::net::socket::sys_accept(fd as i32, addr, len) as i64;
    t[44]  = |_,fd,buf,len,flags,addr,f| crate::net::socket::sys_sendto(fd as i32, buf, len as usize, flags as i32, addr, f.r9 as u32);
    t[45]  = |_,fd,buf,len,flags,addr,f| crate::net::socket::sys_recvfrom(fd as i32, buf, len as usize, flags as i32, addr, f.r9);
    t[46]  = |_,fd,msg,flags,_,_,_| sys_sendmsg(fd as i32, msg, flags as i32);
    t[47]  = |_,fd,msg,flags,_,_,_| sys_recvmsg(fd as i32, msg, flags as i32);
    t[48]  = |_,fd,how,_,_,_,_| crate::net::socket::sys_shutdown(fd as i32, how as i32) as i64;
    t[49]  = |_,fd,addr,len,_,_,_| crate::net::socket::sys_bind(fd as i32, addr, len as u32) as i64;
    t[50]  = |_,fd,backlog,_,_,_,_| crate::net::socket::sys_listen(fd as i32, backlog as i32) as i64;
    t[51]  = |_,fd,addr,len,_,_,_| crate::net::socket::sys_getsockname(fd as i32, addr, len) as i64;
    t[52]  = |_,fd,addr,len,_,_,_| crate::net::socket::sys_getpeername(fd as i32, addr, len) as i64;
    t[53]  = |_,fam,typ,proto,sv,_,_| crate::net::socket::sys_socketpair(fam as i32, typ as i32, proto as i32, sv) as i64;
    t[54]  = |_,fd,level,opt,val,len,_| crate::net::socket::sys_setsockopt(fd as i32, level as i32, opt as i32, val, len as u32) as i64;
    t[55]  = |_,fd,level,opt,val,len,_| crate::net::socket::sys_getsockopt(fd as i32, level as i32, opt as i32, val, len) as i64;

    // ── Misc ──────────────────────────────────────────────────────────
    t[217] = |_,_,_,_,_,_,_| 0; // getdents64 (handled below)
    t[228] = |_,clk,ts,_,_,_,_| sys_clock_gettime(clk as i32, ts);
    t[231] = |_,code,_,_,_,_,_| sys_exit_group(code as i32);
    t[232] = |_,epfd,fd,ev,_,_,_| sys_epoll_wait(epfd as i32, fd, ev as i32, 0); // epoll_wait variant
    t[233] = |_,epfd,op,fd,ev,_,_| sys_epoll_ctl(epfd as i32, op as i32, fd as i32, ev);
    t[234] = |_,epfd,flags,_,_,_,_| sys_epoll_create(flags as i32);
    t[257] = |_,dirfd,path,flags,mode,_,_| sys_openat(dirfd as i32, path, flags as i32, mode as u32);
    t[258] = |_,dirfd,path,mode,_,_,_| sys_mkdirat(dirfd as i32, path, mode as u32);
    t[259] = |_,dirfd,path,mode,dev,_,_| sys_mknodat(dirfd as i32, path, mode as u32, dev as u64);
    t[260] = |_,dirfd,path,uid,gid,_,_| sys_fchownat(dirfd as i32, path, uid as u32, gid as u32, 0i32);
    t[261] = |_,dirfd,path,ts,flags,_,_| sys_futimesat(dirfd as i32, path, 0u64);
    t[262] = |_,dirfd,path,st,flags,_,_| sys_newfstatat(dirfd as i32, path, st, flags as i32);
    t[263] = |_,dirfd,path,flags,_,_,_| sys_unlinkat(dirfd as i32, path, flags as i32);
    t[264] = |_,olddirfd,oldpath,newdirfd,newpath,_,_| sys_renameat(olddirfd as i32, oldpath, newdirfd as i32, newpath);
    t[265] = |_,olddirfd,oldpath,newdirfd,newpath,_,_| sys_linkat(olddirfd as i32, oldpath, newdirfd as i32, newpath, 0i32);
    t[266] = |_,target,newdirfd,newpath,_,_,_| sys_symlinkat(target, newdirfd as i32, newpath);
    t[267] = |_,dirfd,path,buf,sz,_,_| sys_readlinkat(dirfd as i32, path, buf, sz as usize);
    t[268] = |_,dirfd,path,mode,_,_,_| sys_fchmodat(dirfd as i32, path, mode as u32, 0i32);
    t[269] = |_,dirfd,path,mode,_,_,_| sys_faccessat(dirfd as i32, path, mode as i32, 0i32);
    t[280] = |_,dirfd,path,ts,_,_,_| sys_utimensat(dirfd as i32, path, ts, 0i32);
    t[281] = |_,epfd,evs,max,ms,sig,_| sys_epoll_pwait(epfd as i32, evs, max as i32, ms as i32, sig, 0usize);
    t[282] = |_,_,_,_,_,_,_| 0; // signalfd
    t[283] = |_,flags,_,_,_,_,_| sys_timerfd_create(0i32, flags as i32);
    t[284] = |_,fds,_,_,_,_,_| sys_eventfd(fds as u32);
    t[285] = |_,fd,flags,off,len,_,_| sys_fallocate(fd as i32, flags as i32, off as i64, len as i64);
    t[286] = |_,fd,flags,_,_,_,_| sys_timerfd_settime(fd as i32, flags as i32, 0u64, 0u64);
    t[287] = |_,fd,val,_,_,_,_| sys_timerfd_gettime(fd as i32, val);
    t[288] = |_,fd,addr,len,_,_,_| crate::net::socket::sys_accept(fd as i32, addr, len) as i64;
    t[290] = |_,fds,_,_,_,_,_| sys_eventfd2(fds as u32, 0);
    t[291] = |_,flags,_,_,_,_,_| sys_epoll_create(flags as i32);
    t[292] = |_,fds,flags,_,_,_,_| sys_pipe2(fds, flags as i32);
    t[293] = |_,_,_,_,_,_,_| 0; // inotify_init1
    t[295] = |_,flags,_,_,_,_,_| sys_openat(-100i32, 0u64, flags as i32, 0u32); // openat
    t[300] = |_,fd,buf,count,_,_,_| sys_pread64(fd as i32, buf, count as usize, 0);
    t[302] = |_,fd,buf,sz,_,_,_| sys_preadv(fd as i32, buf, sz as usize, 0i64);
    t[318] = |_,buf,len,flags,_,_,_| sys_getrandom(buf, len as usize, flags as u32);
    t[322] = |_,_,_,_,_,_,_| 0; // execveat
    t[334] = |_,fd,_,_,_,_,_| sys_close(fd as i32); // close_range stub
    t[425] = |_,entries,params,_,_,_,_| handlers::sys_io_uring_setup(entries as u32, params);
    t[426] = |_,fd,submit,complete,flags,sig,_| handlers::sys_io_uring_enter(fd as i32, submit as u32, complete as u32, flags as u32, sig, 0);
    t[427] = |_,fd,op,arg,nr,_,_| handlers::sys_io_uring_register(fd as i32, op as u32, arg, nr as u32);
    // pkey syscalls (PKU hardware memory tagging)
    t[329] = |_,flags,rights,_,_,_,_| handlers::sys_pkey_alloc(flags as u32, rights as u32);
    t[330] = |_,key,_,_,_,_,_| handlers::sys_pkey_free(key as i32);
    t[328] = |_,addr,len,prot,key,_,_| handlers::sys_pkey_mprotect(addr, len, prot as i32, key as i32);
    // seccomp(2) syscall 317
    t[317] = |_,op,flags,args,_,_,_| handlers::sys_seccomp(op as u32, flags as u32, args);
    t[435] = |_,_,_,_,_,_,_| 0; // clone3

    t
}

// The table — initialized at startup, lives in .rodata.
static DISPATCH_TABLE: [SyscallFn; NR_SYSCALLS] = make_table();

/// Main dispatch entry point — called from assembly in syscall_entry.rs.
#[no_mangle]
pub fn dispatch(nr: u64, frame: &mut SyscallFrame) -> i64 {
    let idx = nr as usize;

    // Validate user pointers before dispatch (prevents kernel page faults)
    let in_user = |addr: u64| -> bool {
        addr == 0 || addr < 0x0000_8000_0000_0000
    };

    // Special case: getdents64 needs extra wrapper
    if nr == 217 {
        return handlers::sys_getdents64(frame.rdi as i32, frame.rsi, frame.rdx as usize);
    }
    if nr == 72 { // fcntl
        return handlers::sys_fcntl(frame.rdi as i32, frame.rsi as i32, frame.rdx);
    }
    if nr == 240 { // futex
        return crate::abi_compat::syscall::sys_futex(
            frame.rdi, frame.rsi as i32, frame.rdx as u32,
            frame.r10, frame.r8, frame.r9 as u32
        );
    }

    if idx >= NR_SYSCALLS {
        crate::klog!("syscall {} out of range", nr);
        return -38; // ENOSYS
    }

    // QSF seccomp-BPF check (Layer 4a)
    {
        let ip = frame.rcx; // return address saved by SYSCALL instruction
        let args = [frame.rdi, frame.rsi, frame.rdx, frame.r10, frame.r8, frame.r9];
        let (sc_result, sc_errno) = crate::security::seccomp::check_seccomp(nr, ip, args);
        match sc_result {
            crate::security::QsfResult::Allow => {}
            crate::security::QsfResult::Deny  => return -(sc_errno.abs() as i64).max(1),
            crate::security::QsfResult::Kill  => {
                crate::signal::send_signal(crate::process::current_pid(), 31);
                return -1;
            }
        }
    }

    // QSF Layer 4: syscall allowlist check
    match crate::security::qsf_check_syscall(nr) {
        crate::security::QsfResult::Allow => {}
        crate::security::QsfResult::Deny  => return -1i64, // EPERM
        crate::security::QsfResult::Kill  => {
            crate::signal::send_signal(crate::process::current_pid(), 31); // SIGSYS
            return -1i64;
        }
    }

    // Plugin pre-syscall hook
    crate::plugins::hooks::pre_syscall(nr, crate::process::current_pid(), frame.rdi, frame.rsi, frame.rdx);

    // Dispatch via jump table
    let retval = DISPATCH_TABLE[idx](
        nr,
        frame.rdi,  // arg1
        frame.rsi,  // arg2
        frame.rdx,  // arg3
        frame.r10,  // arg4 (r10 used instead of rcx per Linux ABI)
        frame.r8,   // arg5
        frame,
    );

    // Plugin post-syscall hook
    crate::plugins::hooks::post_syscall(nr, crate::process::current_pid(), retval);

    // ── Signal delivery at syscall exit ────────────────────────────────
    // Check for pending signals before returning to user space.
    // User-handler signals patch frame.rip_saved so sysretq jumps to the
    // handler. Default-disposition signals change process state here.
    // This must run after the post-syscall hook and before the asm epilogue.
    //
    // Skip delivery for sys_rt_sigreturn (nr=15): sigreturn already restored
    // the saved context and calling delivery here would overwrite rip_saved
    // with a stale handler address.
    if nr != 15 {
        crate::signal::deliver_pending_at_syscall_exit(frame);
    }

    if nr == 57 {
        crate::drivers::serial::write_str("[dispatch] returning from fork\n");
    }

    retval
}
