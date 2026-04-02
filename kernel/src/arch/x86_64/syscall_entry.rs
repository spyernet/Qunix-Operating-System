use core::arch::global_asm;

// SYSCALL/SYSRET fast path. On entry:
//   rax = syscall number
//   rdi,rsi,rdx,r10,r8,r9 = arguments
//   rcx = return rip (saved by CPU)
//   r11 = saved rflags
//
// GS-base holds a PercpuData:
//   [0]  = kernel rsp (to switch to)
//   [16] = scratch to save user rsp
global_asm!(r#"
.global syscall_entry
syscall_entry:
    swapgs

    // save user rsp, load kernel rsp
    mov    gs:[16], rsp
    mov    rsp, gs:[0]

    // build a SyscallFrame on the kernel stack (same layout as struct SyscallFrame)
    push   rcx          // user rip (return address)
    push   r11          // user rflags
    push   rax          // syscall number
    push   rbx
    push   rcx          // saved again as frame.rcx
    push   rdx
    push   rsi
    push   rdi
    push   rbp
    push   r8
    push   r9
    push   r10
    push   r11          // saved again as frame.r11
    push   r12
    push   r13
    push   r14
    push   r15

    // syscall_dispatch(nr: u64, frame: &mut SyscallFrame) -> i64
    mov    rdi, rax     // nr
    mov    rsi, rsp     // frame pointer
    sub    rsp, 8       // SysV ABI: align stack before calling Rust
    call   syscall_dispatch_rs
    add    rsp, 8

.global syscall_exit
syscall_exit:
    // result in rax; restore registers
    pop    r15
    pop    r14
    pop    r13
    pop    r12
    pop    r11
    pop    r10
    pop    r9
    pop    r8
    pop    rbp
    pop    rdi
    pop    rsi
    pop    rdx
    pop    rcx          // frame.rcx  (discard, rcx_ret is what matters)
    pop    rbx
    add    rsp, 8       // skip frame.rax  (rax already has retval)
    pop    r11          // user rflags -> r11 for sysretq
    pop    rcx          // user rip   -> rcx for sysretq

    // restore user rsp
    mov    rsp, gs:[16]
    swapgs
    sysretq
"#);

#[repr(C)]
pub struct SyscallFrame {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9:  u64, pub r8:  u64,
    pub rbp: u64, pub rdi: u64, pub rsi: u64, pub rdx: u64,
    pub rcx: u64, pub rbx: u64, pub rax: u64,
    pub rflags_saved: u64,
    pub rip_saved:    u64,
}


// Fork child return stub — entered when a freshly-forked child is
// first scheduled. At this point RSP points at the child's SyscallFrame
// (copied from parent, with rax=0 pre-set). We set rax=0 and jump to
// the normal syscall exit path to sysretq back to userspace.
global_asm!(r#"
.global fork_child_return
fork_child_return:
    xor eax, eax          // child's return value from fork() = 0
    jmp syscall_exit      // restore registers and sysretq
"#);

extern "C" {
    pub fn fork_child_return();
}

extern "C" {
    pub fn syscall_entry();
}

#[no_mangle]
pub extern "C" fn syscall_dispatch_rs(nr: u64, frame: &mut SyscallFrame) -> i64 {
    crate::syscall::dispatch(nr, frame)
}
