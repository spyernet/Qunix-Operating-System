//! Process and thread management.
//! Supports both processes (independent address spaces) and
//! threads (shared address space, fds, signals via CLONE_VM).

use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use spin::Mutex;
use crate::memory::vmm::AddressSpace;
use crate::signal::{SignalSet, SigAction};
use crate::vfs::FileDescriptor;

pub type Pid = u32;
pub type Uid = u32;
pub type Gid = u32;

pub const KERNEL_STACK_SIZE: usize = 32 * 1024;
pub const MAX_FDS: usize = 1024;
pub const MAX_PIDS: usize = 65536;

// ── Shared resources between threads ─────────────────────────────────────

/// Resources shared between threads in the same thread group.
/// Created once per process; all threads in the group hold an Arc.
pub struct ThreadGroup {
    pub tgid:     Pid,                              // thread group leader PID
    pub fds:      Mutex<BTreeMap<u32, FileDescriptor>>,
    pub next_fd:  Mutex<u32>,
    pub cwd:      Mutex<String>,
    pub umask:    Mutex<u32>,
    pub sig_actions: Mutex<[SigAction; 32]>,
}

impl ThreadGroup {
    fn new(tgid: Pid, cwd: String) -> Arc<Self> {
        let mut fds = BTreeMap::new();
        Arc::new(ThreadGroup {
            tgid,
            fds:         Mutex::new(fds),
            next_fd:     Mutex::new(3),
            cwd:         Mutex::new(cwd),
            umask:       Mutex::new(0o022),
            sig_actions: Mutex::new([SigAction::default(); 32]),
        })
    }
}

// ── Process state ─────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ProcessState {
    Runnable,
    Running,
    Sleeping,
    Stopped,
    Zombie(i32),
}

// ── CPU context ───────────────────────────────────────────────────────────

#[repr(C)]
pub struct CpuContext {
    pub rbx:    u64,
    pub rbp:    u64,
    pub r12:    u64,
    pub r13:    u64,
    pub r14:    u64,
    pub r15:    u64,
    pub rsp:    u64,
    pub rip:    u64,
    pub rflags: u64,
}

impl CpuContext {
    pub const fn zeroed() -> Self {
        CpuContext { rbx:0, rbp:0, r12:0, r13:0, r14:0, r15:0, rsp:0, rip:0, rflags:0x202 }
    }
}

// ── Process Control Block ─────────────────────────────────────────────────

pub struct Process {
    // Identity
    pub pid:          Pid,
    pub ppid:         Pid,
    pub pgid:         u32,
    pub sid:          u32,
    pub tty:          i32,

    // Thread group — shared between threads, None for kernel threads
    pub thread_group: Option<Arc<ThreadGroup>>,

    // QSF security
    pub mac_label:      crate::security::SecurityLabel,
    pub syscall_filter: crate::security::SyscallFilter,
    pub namespaces:     crate::security::Namespaces,
    pub seccomp:        crate::security::seccomp::FilterChain,
    pub pkey_map:       crate::security::memory_tagging::PkeyAllocMap,
    pub pkru_shadow:    u32,
    pub sw_pkey_table:  crate::security::memory_tagging::SoftwareKeyTable,

    // Scheduling
    pub state:          ProcessState,
    pub context:        CpuContext,
    pub kernel_stack:   Vec<u8>,
    pub nice:           i8,
    pub sleep_until:    u64,
    pub priority:       u8,   // effective priority (0=highest RT, 139=idle)
    pub static_priority:u8,   // base priority before PI boost

    // Memory — threads in the same group share the Arc<Mutex<AddressSpace>>
    pub address_space: AddressSpace,

    // Per-thread file descriptors (fallback when not in thread group)
    pub fds:          BTreeMap<u32, FileDescriptor>,
    pub next_fd:      u32,
    pub cwd:          String,
    pub umask:        u32,

    // Credentials
    pub uid:  Uid, pub gid:  Gid,
    pub euid: Uid, pub egid: Gid,
    pub suid: Uid, pub sgid: Gid,
    pub groups: Vec<Gid>,

    // Signals
    pub sig_pending:  SignalSet,
    pub sig_mask:     SignalSet,
    pub sig_actions:  [SigAction; 32],
    pub sig_altstack: u64,

    // Exit / wait
    pub exit_code:    i32,
    pub children:     Vec<Pid>,

    // Metadata
    pub name:        String,
    pub personality: u32,
    pub flags:       u32,
    pub start_time:  u64,

    // Thread-local storage FS base
    pub fs_base:     u64,
    // Child thread ID address for CLONE_CHILD_CLEARTID
    pub clear_child_tid: u64,
}

pub const PERSONALITY_POSIX: u32 = 0x0000;
/// Alias for PERSONALITY_POSIX (Linux binary ABI is the same)
pub const PERSONALITY_LINUX: u32 = PERSONALITY_POSIX;
pub const PERSONALITY_QUNIX: u32 = 0xFFFF;

impl Process {
    pub fn new_kernel(pid: Pid, entry: u64) -> Self {
        let mut stack = alloc::vec![0u8; KERNEL_STACK_SIZE];
        let rsp = stack.as_mut_ptr() as u64 + KERNEL_STACK_SIZE as u64 - 16;
        let mut ctx = CpuContext::zeroed();
        ctx.rsp = rsp; ctx.rip = entry;
        Process {
            pid, ppid: 0, pgid: pid, sid: pid, tty: -1,
            thread_group: None,
            state:    ProcessState::Runnable,
            context:  ctx,
            kernel_stack: stack,
            nice:     0, sleep_until: 0,
            address_space: AddressSpace::new_kernel(),
            fds: BTreeMap::new(), next_fd: 3,
            cwd: String::from("/"), umask: 0o022,
            uid:0, gid:0, euid:0, egid:0, suid:0, sgid:0, groups: Vec::new(),
            sig_pending: SignalSet::empty(), sig_mask: SignalSet::empty(),
            sig_actions: [SigAction::default(); 32],
            sig_altstack: 0,
            priority: 20, static_priority: 20,
            exit_code: 0, children: Vec::new(),
            name: String::from("kthread"), personality: PERSONALITY_QUNIX,
            mac_label: crate::security::SecurityLabel::KERNEL,
            syscall_filter: crate::security::SyscallFilter::allow_all(),
            namespaces: crate::security::Namespaces::root(),
            seccomp: crate::security::seccomp::FilterChain::new(),
            pkey_map: crate::security::memory_tagging::PkeyAllocMap::new(),
            pkru_shadow: 0,
            sw_pkey_table: crate::security::memory_tagging::SoftwareKeyTable::default(),
            flags: 0, start_time: 0,
            fs_base: 0, clear_child_tid: 0,
        }
    }

    // ── FD helpers ─────────────────────────────────────────────────────

    pub fn alloc_fd(&mut self, fd: FileDescriptor) -> u32 {
        if let Some(tg) = &self.thread_group {
            let mut fds = tg.fds.lock();
            let mut next = tg.next_fd.lock();
            while fds.contains_key(&*next) { *next += 1; if *next as usize >= MAX_FDS { *next = 3; } }
            let n = *next; *next += 1;
            fds.insert(n, fd); n
        } else {
            while self.fds.contains_key(&self.next_fd) {
                self.next_fd += 1;
                if self.next_fd as usize >= MAX_FDS { self.next_fd = 3; }
            }
            let n = self.next_fd; self.next_fd += 1;
            self.fds.insert(n, fd); n
        }
    }

    pub fn alloc_fd_at(&mut self, n: u32, fd: FileDescriptor) -> u32 {
        if let Some(tg) = &self.thread_group { tg.fds.lock().insert(n, fd); }
        else { self.fds.insert(n, fd); }
        n
    }

    pub fn get_fd(&self, n: u32) -> Option<FileDescriptor> {
        if let Some(tg) = &self.thread_group { tg.fds.lock().get(&n).cloned() }
        else { self.fds.get(&n).cloned() }
    }

    pub fn get_fd_mut_op<F, R>(&mut self, n: u32, f: F) -> Option<R>
    where F: FnOnce(&mut FileDescriptor) -> R {
        if let Some(tg) = &self.thread_group {
            let mut fds = tg.fds.lock();
            fds.get_mut(&n).map(f)
        } else {
            self.fds.get_mut(&n).map(f)
        }
    }

    pub fn close_fd(&mut self, n: u32) -> bool {
        if let Some(tg) = &self.thread_group { tg.fds.lock().remove(&n).is_some() }
        else { self.fds.remove(&n).is_some() }
    }

    pub fn fd_keys(&self) -> Vec<u32> {
        if let Some(tg) = &self.thread_group { tg.fds.lock().keys().copied().collect() }
        else { self.fds.keys().copied().collect() }
    }

    pub fn get_cwd(&self) -> String {
        if let Some(tg) = &self.thread_group { tg.cwd.lock().clone() }
        else { self.cwd.clone() }
    }

    pub fn set_cwd(&mut self, cwd: String) {
        if let Some(tg) = &self.thread_group { *tg.cwd.lock() = cwd; }
        else { self.cwd = cwd; }
    }

    pub fn get_sig_action(&self, sig: usize) -> SigAction {
        if let Some(tg) = &self.thread_group { tg.sig_actions.lock()[sig] }
        else { self.sig_actions[sig] }
    }

    pub fn set_sig_action(&mut self, sig: usize, action: SigAction) {
        if let Some(tg) = &self.thread_group { tg.sig_actions.lock()[sig] = action; }
        else { self.sig_actions[sig] = action; }
    }

    pub fn is_zombie(&self) -> bool { matches!(self.state, ProcessState::Zombie(_)) }
    pub fn is_alive(&self)  -> bool { !self.is_zombie() }
    pub fn tgid(&self) -> Pid {
        self.thread_group.as_ref().map(|tg| tg.tgid).unwrap_or(self.pid)
    }
}

// ── Process table ─────────────────────────────────────────────────────────

struct Table {
    procs:    BTreeMap<Pid, Process>,
    next_pid: Pid,
    current:  Pid,
}

impl Table {
    const fn new() -> Self { Table { procs: BTreeMap::new(), next_pid: 1, current: 0 } }

    fn alloc_pid(&mut self) -> Option<Pid> {
        let start = self.next_pid;
        loop {
            let pid = self.next_pid;
            self.next_pid = if self.next_pid >= MAX_PIDS as Pid { 2 } else { self.next_pid + 1 };
            if !self.procs.contains_key(&pid) { return Some(pid); } // fixed: was bare `return pid`
            if self.next_pid == start { return None; } // PID space exhausted
        }
    }
}

static TABLE: Mutex<Table> = Mutex::new(Table::new());

// ── Public API ────────────────────────────────────────────────────────────

pub fn init() {
    let mut t = TABLE.lock();
    let pid   = t.alloc_pid().expect("PID space exhausted at init");
    let mut proc = Process::new_kernel(pid, idle_thread as u64);
    proc.start_time = 0;
    t.procs.insert(pid, proc);
    t.current = pid;
    crate::klog!("Process: table init, idle pid={}", pid);
}

pub fn current_pid() -> Pid { TABLE.lock().current }
pub fn set_current(pid: Pid) {
    // Acquire once — set current and read kernel stack top in a single lock scope.
    // A second TABLE.lock() in the same function would deadlock on a non-reentrant spinlock.
    let kstack_top = {
        let mut t = TABLE.lock();
        t.current = pid;
        t.procs.get(&pid)
            .map(|p| p.kernel_stack.as_ptr() as u64 + KERNEL_STACK_SIZE as u64)
            .unwrap_or(0)
    }; // lock dropped here
    if kstack_top != 0 {
        crate::arch::x86_64::gdt::set_kernel_stack(kstack_top);
        crate::arch::x86_64::smp::set_kernel_stack(kstack_top);
    }
}

/// Access the current process. Returns f's result.
/// If no process is current (should not happen in normal kernel operation),
/// silently returns None.
pub fn with_current<F, R>(f: F) -> Option<R> where F: FnOnce(&Process) -> R {
    let t = TABLE.lock(); let pid = t.current;
    t.procs.get(&pid).map(f)
}

pub fn with_current_mut<F, R>(f: F) -> Option<R> where F: FnOnce(&mut Process) -> R {
    let mut t = TABLE.lock(); let pid = t.current;
    t.procs.get_mut(&pid).map(f)
}

pub fn with_process<F, R>(pid: Pid, f: F) -> Option<R> where F: FnOnce(&Process) -> R {
    TABLE.lock().procs.get(&pid).map(f)
}

pub fn with_process_mut<F, R>(pid: Pid, f: F) -> Option<R> where F: FnOnce(&mut Process) -> R {
    TABLE.lock().procs.get_mut(&pid).map(f)
}

/// Spawn process. If proc.pid != 0, tries to use that PID (for init=1).
pub fn spawn(mut proc: Process) -> Pid {
    let mut t = TABLE.lock();
    let pid = if proc.pid != 0 && !t.procs.contains_key(&proc.pid) {
        proc.pid
    } else {
        match t.alloc_pid() {
            Some(p) => p,
            None    => return 0, // PID space exhausted — caller must check
        }
    };
    proc.pid = pid;
    if proc.ppid != 0 && proc.ppid != pid {
        if let Some(parent) = t.procs.get_mut(&proc.ppid) {
            if !parent.children.contains(&pid) { parent.children.push(pid); }
        }
    }
    t.procs.insert(pid, proc);
    pid
}

/// Fork: create a child process with copied address space and file descriptors.
///
/// ## Scheduling contract
/// This function inserts the child into the process table but does NOT schedule it.
/// The caller MUST call `crate::sched::add_task(child_pid, ...)` before returning
/// to user space, otherwise the child will never run.
///
/// This invariant is enforced at every call site:
///   - `sys_fork()` calls `add_task(child)` immediately after `fork_current()`
///   - `clone_thread()` similarly requires the caller to schedule
pub fn fork_current() -> Option<Pid> {
    // Mask local IRQs while holding the global process-table lock. The timer
    // interrupt also touches process state, so taking TABLE.lock() in an IRQ
    // while fork() already holds it would deadlock on a single CPU.
    let _irq_guard = crate::arch::x86_64::cpu::IrqGuard::new();
    let mut t = TABLE.lock();
    let ppid  = t.current;
    let child_pid = t.alloc_pid()?; // returns None → fork fails with ENOMEM
    let parent = t.procs.get_mut(&ppid)?;

    // Deep-copy address space
    let child_as = parent.address_space.copy_on_fork()?;

    // Copy FDs — clone the entire fd table so the child inherits all open files.
    // For pipe fds, increment the pipe's reader/writer refcount so that
    // close() in either parent or child only marks the pipe closed when the
    // LAST holder calls close, not the first.
    let child_fds: BTreeMap<u32, FileDescriptor> = if let Some(tg) = &parent.thread_group {
        tg.fds.lock().clone()
    } else {
        parent.fds.clone()
    };
    // Increment pipe refcounts for every inherited pipe fd
    for (_, fd) in &child_fds {
        match &fd.kind {
            crate::vfs::FdKind::PipeWrite(pipe) => { pipe.lock().dup_write(); }
            crate::vfs::FdKind::PipeRead(pipe)  => { pipe.lock().dup_read();  }
            _ => {}
        }
    }
    let child_next_fd = if let Some(tg) = &parent.thread_group {
        *tg.next_fd.lock()
    } else { parent.next_fd };
    let child_cwd = parent.get_cwd();

    let mut kstack = alloc::vec![0u8; KERNEL_STACK_SIZE];
    let kst = kstack.as_mut_ptr() as u64 + KERNEL_STACK_SIZE as u64;

    // Build the child's initial kernel stack so that context_switch can
    // pick it up correctly and the child returns to userspace with rax=0.
    //
    // Stack layout (growing DOWN from kst):
    //   [kst-136 .. kst]       SyscallFrame copy (parent's, rax overwritten to 0)
    //   [kst-144]              address of fork_child_return stub
    //   [kst-192 .. kst-144]   6 callee-saved zeros (popped by context_switch)
    //
    // child.context.rsp = kst - 192
    const SYSCALL_FRAME_SIZE: usize = 136; // 17 fields × 8 bytes

    // Copy parent's SyscallFrame from top of parent's kernel stack
    let parent_frame_start = parent.kernel_stack.as_ptr() as usize
        + KERNEL_STACK_SIZE - SYSCALL_FRAME_SIZE;
    unsafe {
        core::ptr::copy_nonoverlapping(
            parent_frame_start as *const u8,
            (kst as usize - SYSCALL_FRAME_SIZE) as *mut u8,
            SYSCALL_FRAME_SIZE,
        );
        // SyscallFrame.rax is field index 14 from the start (r15..rax = 15 fields,
        // so rax is at offset 14*8 = 112 from the frame start).
        // Set it to 0 so the child's fork() returns 0.
        let rax_offset = kst as usize - SYSCALL_FRAME_SIZE + 14 * 8;
        *(rax_offset as *mut u64) = 0u64;

        // Write fork_child_return address at [kst-144]
        let stub_ptr = crate::arch::x86_64::syscall_entry::fork_child_return as u64;
        *((kst as usize - SYSCALL_FRAME_SIZE - 8) as *mut u64) = stub_ptr;

        // 6 callee-saved slots are already zero (kstack is zeroed)
    }

    let child_krsp = kst - SYSCALL_FRAME_SIZE as u64 - 8 - 48; // = kst - 192
    crate::drivers::serial::write_str("[fork] pre-child-struct\n");
    let child = Process {
        pid: child_pid, ppid, pgid: parent.pgid, sid: parent.sid, tty: parent.tty,
        thread_group: None, // child gets its own thread group
        state:  ProcessState::Runnable,
        context: CpuContext {
            rbx: 0, rbp: 0, r12: 0, r13: 0, r14: 0, r15: 0,
            rsp: child_krsp,
            rip: 0, // child resumes via fork_child_return, not rip
            rflags: 0,
        },
        kernel_stack: kstack,
        nice: parent.nice, sleep_until: 0,
        address_space: child_as,
        fds: child_fds, next_fd: child_next_fd,
        cwd: child_cwd, umask: parent.umask,
        uid: parent.uid, gid: parent.gid,
        euid: parent.euid, egid: parent.egid,
        suid: parent.suid, sgid: parent.sgid,
        groups: parent.groups.clone(),
        sig_pending: SignalSet::empty(), sig_mask: parent.sig_mask,
        sig_actions: parent.sig_actions, sig_altstack: parent.sig_altstack,
        exit_code: 0, children: Vec::new(),
        name: parent.name.clone(), personality: parent.personality,
        flags: parent.flags, start_time: crate::time::ticks(),
        fs_base: parent.fs_base, clear_child_tid: 0,
        mac_label: parent.mac_label,
        syscall_filter: parent.syscall_filter.clone(),
        namespaces: parent.namespaces.clone(),
        seccomp: parent.seccomp.clone(),
        pkey_map: parent.pkey_map.clone(),
        pkru_shadow: parent.pkru_shadow,
        sw_pkey_table: parent.sw_pkey_table.clone(),
        priority: parent.priority, static_priority: parent.static_priority,
    };

    crate::drivers::serial::write_str("[fork] post-child-struct\n");
    if let Some(p) = t.procs.get_mut(&ppid) {
        crate::drivers::serial::write_str("[fork] pre-parent-push\n");
        p.children.push(child_pid);
        crate::drivers::serial::write_str("[fork] post-parent-push\n");
    }
    crate::drivers::serial::write_str("[fork] pre-proc-insert\n");
    t.procs.insert(child_pid, child);
    crate::drivers::serial::write_str("[fork] post-proc-insert\n");
    Some(child_pid)
}

/// Clone with thread semantics: shares address space and FDs.
/// Used when CLONE_VM | CLONE_FILES | CLONE_THREAD are set.
pub fn clone_thread(
    parent_pid: Pid,
    child_stack: u64,   // user-space stack for the new thread
    tls_value: u64,     // FS.base for thread-local storage
    child_tid_ptr: u64, // address to write child tid (CLONE_CHILD_SETTID)
) -> Option<Pid> {
    let _irq_guard = crate::arch::x86_64::cpu::IrqGuard::new();
    let mut t = TABLE.lock();
    let child_pid = t.alloc_pid()?;

    let parent = t.procs.get(&parent_pid)?;

    // Thread group — share or create
    let tg = if let Some(existing) = &parent.thread_group {
        existing.clone()
    } else {
        // Promote parent to have a thread group
        let new_tg = ThreadGroup::new(parent_pid, parent.get_cwd());
        // Copy parent fds into thread group
        {
            let mut tg_fds = new_tg.fds.lock();
            *tg_fds = parent.fds.clone();
            *new_tg.next_fd.lock() = parent.next_fd;
        }
        new_tg
    };

    // New kernel stack for the thread
    let mut kstack = alloc::vec![0u8; KERNEL_STACK_SIZE];
    let kst = kstack.as_mut_ptr() as u64 + KERNEL_STACK_SIZE as u64;

    // Same child stack setup as fork: copy SyscallFrame, set rax=0.
    // Additionally override the saved user RSP with child_stack.
    const SYSCALL_FRAME_SIZE_T: usize = 136;
    let parent_frame_start = parent.kernel_stack.as_ptr() as usize
        + KERNEL_STACK_SIZE - SYSCALL_FRAME_SIZE_T;
    unsafe {
        core::ptr::copy_nonoverlapping(
            parent_frame_start as *const u8,
            (kst as usize - SYSCALL_FRAME_SIZE_T) as *mut u8,
            SYSCALL_FRAME_SIZE_T,
        );
        // Set rax=0 (field 14, offset 112)
        let rax_off = kst as usize - SYSCALL_FRAME_SIZE_T + 14 * 8;
        *(rax_off as *mut u64) = 0u64;
        // Adjust saved user RSP to child_stack if provided
        if child_stack != 0 {
            // gs:[16] stores user rsp; the SyscallFrame doesn't store it directly
            // because syscall_entry saves user rsp into gs:[16] before switching stacks.
            // We store child_stack in Process.context.rsp (user-mode rsp) which the
            // context-switch path will write to gs:[16] before sysretq.
            // (handled below in Process struct via a new user_rsp field)
        }
        // Write stub address
        let stub_ptr = crate::arch::x86_64::syscall_entry::fork_child_return as u64;
        *((kst as usize - SYSCALL_FRAME_SIZE_T - 8) as *mut u64) = stub_ptr;
    }
    let child_krsp = kst - SYSCALL_FRAME_SIZE_T as u64 - 8 - 48;

    // The thread starts at the return point of the clone() syscall.
    // child_stack is the user-space stack. The thread will return 0 from clone.
    let child = Process {
        pid: child_pid,
        ppid: parent_pid,
        pgid: parent.pgid,
        sid:  parent.sid,
        tty:  parent.tty,
        thread_group: Some(tg),
        state:  ProcessState::Runnable,
        context: CpuContext {
            rbx: parent.context.rbx, rbp: parent.context.rbp,
            r12: parent.context.r12, r13: parent.context.r13,
            r14: parent.context.r14, r15: parent.context.r15,
            rsp: child_krsp,
            rip: 0,  // child resumes via fork_child_return stub
            rflags: 0,
        },
        kernel_stack: kstack,
        nice: parent.nice, sleep_until: 0,
        // CLONE_VM: share same PML4 — no copy, use parent's physical root
        address_space: AddressSpace::shared(parent.address_space.pml4_phys),
        fds: BTreeMap::new(),   // accessed via thread_group.fds
        next_fd: 0,             // accessed via thread_group.next_fd
        cwd: String::new(),     // accessed via thread_group.cwd
        umask: parent.umask,
        uid: parent.uid, gid: parent.gid,
        euid: parent.euid, egid: parent.egid,
        suid: parent.suid, sgid: parent.sgid,
        groups: parent.groups.clone(),
        sig_pending: SignalSet::empty(),
        sig_mask: parent.sig_mask,
        sig_actions: parent.sig_actions,
        sig_altstack: parent.sig_altstack,
        exit_code: 0, children: Vec::new(),
        name: parent.name.clone(), personality: parent.personality,
        flags: parent.flags, start_time: crate::time::ticks(),
        fs_base: tls_value,
        mac_label: parent.mac_label,
        syscall_filter: parent.syscall_filter.clone(),
        namespaces: parent.namespaces.clone(),
        seccomp: parent.seccomp.clone(),
        pkey_map: parent.pkey_map.clone(),
        pkru_shadow: parent.pkru_shadow,
        sw_pkey_table: parent.sw_pkey_table.clone(),
        priority: parent.priority, static_priority: parent.static_priority,
        clear_child_tid: child_tid_ptr,
    };

    // If we just created a new thread group, update parent too
    // (We do this outside the borrow by storing the tg first)
    // Note: parent borrow already dropped above when we did parent.xxx

    if let Some(p) = t.procs.get_mut(&parent_pid) {
        p.children.push(child_pid);
        // Ensure parent also uses thread group
        if p.thread_group.is_none() {
            // Re-fetch the tg we created (it's in the child)
        }
    }
    t.procs.insert(child_pid, child);

    // Write child TID to the child_tid_ptr in its stack
    if child_tid_ptr != 0 {
        // Safe write into shared address space
        unsafe {
            let phys = crate::arch::x86_64::paging::PageMapper::new(
                t.procs[&child_pid].address_space.pml4_phys
            ).translate(child_tid_ptr);
            if let Some(p) = phys {
                let virt = crate::arch::x86_64::paging::phys_to_virt(p & !0xFFF);
                *((virt + (child_tid_ptr & 0xFFF)) as *mut u32) = child_pid;
            }
        }
    }

    Some(child_pid)
}

pub fn exit_current(code: i32) {
    let pid;
    let ppid;
    let ctid;
    let pml4;

    // Close all pipe file descriptors before marking zombie.
    // Deduplication by Arc pointer ensures pipe_close_* is called exactly once
    // per unique pipe end even when fds are dup'd or shared via thread groups.
    {
        use alloc::collections::BTreeSet;
        use alloc::sync::Arc;
        use spin::Mutex;
        use crate::ipc::pipe::PipeBuf;

        let mut seen: BTreeSet<usize> = BTreeSet::new();
        let mut write_pipes: alloc::vec::Vec<Arc<Mutex<PipeBuf>>> = alloc::vec::Vec::new();
        let mut read_pipes:  alloc::vec::Vec<Arc<Mutex<PipeBuf>>> = alloc::vec::Vec::new();

        {
            let t = TABLE.lock();
            let pid_val = t.current;
            if let Some(proc) = t.procs.get(&pid_val) {
                // Drain a fd map into the dedup-tracked pipe lists
                let mut add_fds = |fds: &alloc::collections::BTreeMap<u32, crate::vfs::FileDescriptor>| {
                    for fd in fds.values() {
                        match &fd.kind {
                            crate::vfs::FdKind::PipeWrite(arc) => {
                                let key = (Arc::as_ptr(arc) as usize) | 1;
                                if seen.insert(key) { write_pipes.push(arc.clone()); }
                            }
                            crate::vfs::FdKind::PipeRead(arc) => {
                                let key = Arc::as_ptr(arc) as usize & !1usize;
                                if seen.insert(key) { read_pipes.push(arc.clone()); }
                            }
                            _ => {}
                        }
                    }
                };
                add_fds(&proc.fds);
                if let Some(ref tg) = proc.thread_group {
                    // clone the map out to avoid holding two locks simultaneously
                    let snapshot: alloc::collections::BTreeMap<u32, crate::vfs::FileDescriptor>
                        = tg.fds.lock().clone();
                    add_fds(&snapshot);
                }
            }
        } // TABLE lock dropped before calling pipe_close_*

        for pipe in write_pipes { crate::ipc::pipe::pipe_close_write(&pipe); }
        for pipe in read_pipes  { crate::ipc::pipe::pipe_close_read(&pipe);  }
    }

    {
        let mut t = TABLE.lock();
        pid  = t.current;
        let proc = match t.procs.get_mut(&pid) { Some(p) => p, None => return };
        proc.state     = ProcessState::Zombie(code);
        proc.exit_code = code;
        ctid = proc.clear_child_tid;
        pml4 = proc.address_space.pml4_phys;
        ppid = proc.ppid;
    }

    // CLONE_CHILD_CLEARTID: write 0 to the tidptr so pthread_join sees it
    if ctid != 0 {
        unsafe {
            let phys = crate::arch::x86_64::paging::PageMapper::new(pml4)
                .translate(ctid);
            if let Some(p) = phys {
                let virt = crate::arch::x86_64::paging::phys_to_virt(p & !0xFFF);
                *((virt + (ctid & 0xFFF)) as *mut u32) = 0;
            }
        }
        crate::abi_compat::syscall::futex_cleanup(pid);
    }

    // Always notify the parent — regardless of ctid
    if ppid != 0 {
        crate::signal::send_signal(ppid, crate::signal::SIGCHLD);
        // Wake parent if it is blocked in wait
        crate::sched::wake_process(ppid);
    }
}

pub fn reap_child(child_pid: Pid) -> Option<i32> {
    let mut t = TABLE.lock();
    if let Some(p) = t.procs.get(&child_pid) {
        if let ProcessState::Zombie(code) = p.state {
            let ppid = p.ppid;
            t.procs.remove(&child_pid);
            if let Some(parent) = t.procs.get_mut(&ppid) {
                parent.children.retain(|&c| c != child_pid);
            }
            return Some(code);
        }
    }
    None
}

pub fn all_pids() -> Vec<Pid> { TABLE.lock().procs.keys().copied().collect() }

pub fn wait_any_zombie(ppid: Pid) -> Option<Pid> {
    let t = TABLE.lock();
    let children = t.procs.get(&ppid).map(|p| p.children.clone())?;
    children.iter().find(|&&c| t.procs.get(&c).map(|p| p.is_zombie()).unwrap_or(false)).copied()
}

pub fn parent_pid_of(pid: Pid) -> Option<Pid> { with_process(pid, |p| p.ppid) }

pub fn kill_current(sig: u32) {
    let pid = TABLE.lock().current;
    crate::signal::send_signal(pid, sig);
}

extern "C" fn idle_thread() -> ! {
    loop {
        crate::arch::x86_64::cpu::enable_interrupts();
        unsafe { core::arch::asm!("hlt"); }
    }
}
