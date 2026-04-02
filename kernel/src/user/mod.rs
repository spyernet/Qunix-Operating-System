//! Userland launch — find and exec /sbin/init from the VFS.
//! Falls back to /bin/qshell, then idle loop.

use alloc::string::String;
use alloc::vec;

pub fn launch_init() {
    // Try init paths in order
    for path in &["/sbin/init", "/bin/init", "/bin/qshell", "/bin/sh"] {
        if try_exec_path(path) {
            unreachable!("exec returned");
        }
    }

    crate::klog!("FATAL: no userland binary found in VFS");
    crate::klog!("  Make sure the disk image contains /sbin/init or /bin/qshell");
    crate::klog!("  Entering idle loop. System halt.");
    idle_loop();
}

fn try_exec_path(path: &str) -> bool {
    let cwd = String::from("/");
    let fd  = match crate::vfs::open(&cwd, path, 0, 0) {
        Ok(f)  => f,
        Err(_) => return false,
    };

    let mut data = alloc::vec![];
    crate::vfs::read_all_fd(&fd, &mut data);
    if data.len() < 64 { return false; }

    // Verify ELF magic
    if &data[0..4] != b"\x7fELF" { return false; }

    crate::klog!("Qunix exec: {} ({} bytes)", path, data.len());

    let argv = vec![String::from(path)];
    let envp = vec![
        String::from("PATH=/bin:/sbin:/usr/bin:/usr/sbin"),
        String::from("HOME=/root"),
        String::from("TERM=xterm-256color"),
        String::from("LANG=en_US.UTF-8"),
        String::from("USER=root"),
        String::from("LOGNAME=root"),
        String::from("SHELL=/bin/qshell"),
        String::from("PWD=/"),
    ];

    match crate::elf::exec(data, argv, envp) {
        Ok(result) => {
            crate::klog!("user: exec parsed, preparing PID 1");
            // Create PID 1 — the init process
            let mut proc = crate::process::Process::new_kernel(1, result.entry);
            proc.name          = String::from(path);
            proc.address_space = result.address_space;
            proc.context.rip   = result.entry;
            proc.context.rsp   = result.stack_top;
            proc.cwd           = String::from("/");
            proc.personality   = crate::abi_compat::PERSONALITY_QUNIX;
            proc.fs_base       = result.tls_addr;

            // Wire stdin/stdout/stderr to /dev/tty
            wire_stdio(&mut proc);

            // Register and schedule
            let pid = crate::process::spawn(proc);
            crate::klog!("user: spawned pid {}", pid);
            crate::sched::add_task(pid, crate::sched::PRIO_NORMAL, crate::sched::SCHED_NORMAL);
            crate::process::set_current(pid);
            crate::klog!("user: switched current to pid {}", pid);

            // Set kernel stack in:
            // 1. TSS rsp0 — used by hardware on interrupt/exception from ring 3
            // 2. GS:kernel_rsp — used by SYSCALL entry assembly to switch stacks
            let (kstack_top, pml4_phys) = crate::process::with_current(|p| (
                p.kernel_stack.as_ptr() as u64 + crate::process::KERNEL_STACK_SIZE as u64,
                p.address_space.pml4_phys,
            )).unwrap_or((0, 0));
            crate::arch::x86_64::gdt::set_kernel_stack(kstack_top);
            crate::arch::x86_64::smp::set_kernel_stack(kstack_top);
            crate::klog!(
                "user: entering ring3 rip={:#x} rsp={:#x} kstack={:#x} cr3={:#x}",
                result.entry,
                result.stack_top,
                kstack_top,
                pml4_phys,
            );

            unsafe { jump_to_user_with_cr3(result.entry, result.stack_top, kstack_top, pml4_phys) }
        }
        Err(e) => {
            crate::klog!("ELF failed {}: error {}", path, e);
            false
        }
    }
}

fn wire_stdio(proc: &mut crate::process::Process) {
    use crate::vfs::{FileDescriptor, FdKind, Inode, InodeOps, Superblock,
                     DirEntry, VfsError, S_IFCHR};
    use alloc::sync::Arc;

    struct TtyOps;
    impl InodeOps for TtyOps {
        fn read(&self,_:&Inode,buf:&mut[u8],_:u64)->Result<usize,VfsError>{
            crate::device::read_device(2, buf.as_mut_ptr(), buf.len(), 0)
        }
        fn write(&self,_:&Inode,buf:&[u8],_:u64)->Result<usize,VfsError>{
            crate::device::write_device(2, buf.as_ptr(), buf.len(), 0)
        }
        fn readdir(&self,_:&Inode,_:u64)->Result<alloc::vec::Vec<DirEntry>,VfsError>{Err(crate::vfs::ENOTDIR)}
        fn lookup(&self,_:&Inode,_:&str)->Result<Inode,VfsError>{Err(crate::vfs::ENOENT)}
    }
    struct NullSb;
    impl crate::vfs::SuperblockOps for NullSb {
        fn get_root(&self)->Result<Inode,VfsError>{Err(crate::vfs::ENOENT)}
    }

    let make_tty_fd = || FileDescriptor {
        inode: Inode {
            ino: 0, mode: S_IFCHR | 0o666, uid:0, gid:0, size:0,
            atime:0, mtime:0, ctime:0,
            ops: Arc::new(TtyOps),
            sb:  Arc::new(Superblock {
                dev: 3, fs_type: alloc::string::String::from("devfs"),
                ops: Arc::new(NullSb),
            }),
        },
        offset: 0, flags: 2,
        kind: FdKind::Device(2),
     path: alloc::string::String::new(),};

    proc.fds.insert(0, make_tty_fd());
    proc.fds.insert(1, make_tty_fd());
    proc.fds.insert(2, make_tty_fd());
    proc.next_fd = 3;
}

fn idle_loop() -> ! {
    crate::klog!("Idle loop — HLT");
    loop {
        crate::arch::x86_64::cpu::enable_interrupts();
        unsafe { core::arch::asm!("hlt"); }
    }
}

unsafe fn jump_to_user_with_cr3(entry: u64, stack: u64, kernel_stack: u64, pml4_phys: u64) -> ! {
    use crate::arch::x86_64::gdt::{USER_CODE_SEL, USER_DATA_SEL};
    core::arch::asm!(
        "mov rsp, {krsp}",
        "mov cr3, {cr3}",
        "mov ax, {uds}",   // removed :x modifier (not allowed for const)
        "mov ds, ax",
        "mov es, ax",
        "push {uds}",
        "push {rsp}",
        "pushfq",
        "pop  rax",
        "or   rax, 0x200",
        "push rax",
        "push {ucs}",
        "push {rip}",
        "iretq",
        uds = const USER_DATA_SEL as u64,
        ucs = const USER_CODE_SEL as u64,
        krsp = in(reg) kernel_stack,
        cr3 = in(reg) pml4_phys,
        rsp = in(reg) stack,
        rip = in(reg) entry,
        options(noreturn)
    );
}
