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
    crate::drivers::serial::write_str("[launch_init] enter\n");
    let cwd = String::from("/");
    let fd  = match crate::vfs::open(&cwd, path, 0, 0) {
        Ok(f)  => f,
        Err(_) => return false,
    };
    crate::drivers::serial::write_str("[launch_init] open ok\n");

    if fd.inode.size < 64 {
        return false;
    }

    let mut magic = [0u8; 4];
    if fd.inode.ops.read(&fd.inode, &mut magic, 0).ok() != Some(4) {
        return false;
    }
    crate::drivers::serial::write_str("[launch_init] magic read ok\n");
    if &magic != b"\x7fELF" {
        return false;
    }
    crate::drivers::serial::write_str("[launch_init] magic ok\n");

    crate::klog!("Qunix exec: {} ({} bytes)", path, fd.inode.size);

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
    crate::drivers::serial::write_str("[launch_init] pre-exec-fd\n");

    match crate::elf::exec_fd(&fd, argv, envp) {
        Ok(result) => {
            crate::drivers::serial::write_str("[launch_init] exec-fd ok\n");
            crate::klog!("user: exec parsed, preparing PID 1");
            crate::drivers::serial::write_str("[user] pre-new-kernel\n");
            // Create PID 1 — the init process
            let mut proc = crate::process::Process::new_kernel(1, result.entry);
            crate::drivers::serial::write_str("[user] post-new-kernel\n");
            proc.name          = String::from(path);
            crate::drivers::serial::write_str("[user] post-name\n");
            proc.address_space = result.address_space;
            crate::drivers::serial::write_str("[user] post-address-space\n");
            proc.context.rip   = result.entry;
            proc.context.rsp   = result.stack_top;
            proc.cwd           = String::from("/");
            proc.personality   = crate::abi_compat::PERSONALITY_QUNIX;
            proc.fs_base       = result.tls_addr;

            // Wire stdin/stdout/stderr to /dev/tty
            crate::drivers::serial::write_str("[user] pre-stdio\n");
            wire_stdio(&mut proc);
            crate::drivers::serial::write_str("[user] post-stdio\n");

            // Register and schedule
            crate::drivers::serial::write_str("[user] pre-spawn\n");
            let pid = crate::process::spawn(proc);
            crate::drivers::serial::write_str("[user] post-spawn\n");
            crate::drivers::serial::write_str("[user] spawned\n");
            crate::drivers::serial::write_str("[user] pre-add-task\n");
            crate::sched::add_task(pid, crate::sched::PRIO_NORMAL, crate::sched::SCHED_NORMAL);
            crate::drivers::serial::write_str("[user] post-add-task\n");
            crate::process::set_current(pid);
            crate::drivers::serial::write_str("[user] post-set-current\n");

            // Set kernel stack in:
            // 1. TSS rsp0 — used by hardware on interrupt/exception from ring 3
            // 2. GS:kernel_rsp — used by SYSCALL entry assembly to switch stacks
            let (kstack_top, pml4_phys) = crate::process::with_current(|p| (
                (p.kernel_stack.as_ptr() as u64 + crate::process::KERNEL_STACK_SIZE as u64) & !0xFu64,
                p.address_space.pml4_phys,
            )).unwrap_or((0, 0));
            crate::arch::x86_64::gdt::set_kernel_stack(kstack_top);
            crate::arch::x86_64::smp::set_kernel_stack(kstack_top);
            crate::drivers::serial::write_str("[user] pre-ring3\n");

            unsafe {
                jump_to_user_with_cr3(
                    result.entry,
                    result.stack_top,
                    result.argc,
                    result.argv_ptr,
                    result.envp_ptr,
                    kstack_top,
                    pml4_phys,
                )
            }
        }
        Err(e) => {
            crate::klog!("ELF failed {}: error {}", path, e);
            false
        }
    }
}

fn wire_stdio(proc: &mut crate::process::Process) {
    // Open /dev/tty through the VFS so we get a properly backed file descriptor
    // with correct devfs InodeOps. The old hardcoded device::read_device(2,...)
    // used the wrong device minor, causing every read(stdin) to return ENODEV
    // so qshell saw immediate EOF and exited.
    let cwd = alloc::string::String::from("/");
    for fd_num in 0u32..3 {
        match crate::vfs::open(&cwd, "/dev/tty", 2, 0) {
            Ok(fd) => { proc.fds.insert(fd_num, fd); }
            Err(_) => {
                if let Ok(fd) = crate::vfs::open(&cwd, "/dev/console", 2, 0) {
                    proc.fds.insert(fd_num, fd);
                }
            }
        }
    }
    proc.next_fd = 3;
}

fn idle_loop() -> ! {
    crate::klog!("Idle loop — HLT");
    loop {
        crate::arch::x86_64::cpu::enable_interrupts();
        unsafe { core::arch::asm!("hlt"); }
    }
}

unsafe fn jump_to_user_with_cr3(
    entry: u64,
    stack: u64,
    argc: u64,
    argv: u64,
    envp: u64,
    kernel_stack: u64,
    pml4_phys: u64,
) -> ! {
    use crate::arch::x86_64::gdt::{USER_CODE_SEL, USER_DATA_SEL};
    // Bug fixes in iretq frame construction:
    //
    // 1. The original code did "mov rsp, {krsp}" then "push {rsp}" — but {rsp}
    //    was bound to the 'stack' (user RSP) variable.  After "mov rsp, {krsp}"
    //    the rsp REGISTER holds the kernel stack pointer, but the compiler-chosen
    //    register for the 'rsp' operand may have been clobbered or been rsp itself.
    //    We use a separate named operand `ursp` to hold the user stack safely.
    //
    // 2. "mov ax, {uds}" requires the :x suffix on the const operand when the
    //    destination is a 16-bit register; without it the assembler infers the
    //    wrong operand size and emits a REX prefix that corrupts ax.
    //
    // 3. Interrupts must be re-enabled in EFLAGS so the scheduler timer fires
    //    once we are in user space. We set IF in the pushed RFLAGS.
    //
    // iretq stack frame (top of stack at iretq time, low address first):
    //   RIP, CS, RFLAGS, RSP (user), SS
    // Load segment selector values into registers so we can use :x modifier
    // on reg operands (not allowed on const operands per rustc).
    core::arch::asm!(
        // Switch to kernel stack so the iretq frame is built there
        "mov rsp, r11",
        // Switch page tables to the new process address space
        "mov cr3, r10",
        // Load user data segment into ds/es via ax.
        // Do NOT touch FS or GS — their bases are MSR-controlled and must stay
        // as kernel percpu until swapgs at the next syscall entry.
        "mov ax, {uds}",
        "mov ds, ax",
        "mov es, ax",
        // Build iretq frame on the kernel stack: SS | RSP | RFLAGS | CS | RIP
        "push {uds}",          // SS  — user data segment
        "push r8",             // RSP — user stack
        "pushfq",              // RFLAGS base
        "pop  rax",
        "or   rax, 0x200",     // set IF — enable interrupts in user space
        "and  rax, 0xFFFFFFFFFFFFFEFF",  // clear TF (no single-step)
        "push rax",            // RFLAGS
        "push {ucs}",          // CS
        "push r9",             // RIP
        "iretq",
        uds  = const USER_DATA_SEL as u64,
        ucs  = const USER_CODE_SEL as u64,
        in("rdi") argc,
        in("rsi") argv,
        in("rdx") envp,
        in("r8") stack,
        in("r9") entry,
        in("r10") pml4_phys,
        in("r11") kernel_stack,
        options(noreturn)
    );
}
