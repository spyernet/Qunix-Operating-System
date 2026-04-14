/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::vec::Vec;

// POSIX/Linux x86-64 ABI constants (binary-stable, do not reorder)
pub const AT_NULL:    u64 = 0;
pub const AT_IGNORE:  u64 = 1;
pub const AT_EXECFD:  u64 = 2;
pub const AT_PHDR:    u64 = 3;
pub const AT_PHENT:   u64 = 4;
pub const AT_PHNUM:   u64 = 5;
pub const AT_PAGESZ:  u64 = 6;
pub const AT_BASE:    u64 = 7;
pub const AT_FLAGS:   u64 = 8;
pub const AT_ENTRY:   u64 = 9;
pub const AT_UID:     u64 = 11;
pub const AT_EUID:    u64 = 12;
pub const AT_GID:     u64 = 13;
pub const AT_EGID:    u64 = 14;
pub const AT_CLKTCK:  u64 = 17;
pub const AT_PLATFORM: u64 = 15;
pub const AT_HWCAP:   u64 = 16;
pub const AT_SECURE:  u64 = 23;
pub const AT_RANDOM:  u64 = 25;
pub const AT_HWCAP2:  u64 = 26;
pub const AT_EXECFN:  u64 = 31;
pub const AT_SYSINFO_EHDR: u64 = 33;

// Build an auxiliary vector for the initial stack of a Linux process
pub fn build_auxv(
    stack_top: &mut u64,
    entry: u64,
    phdr: u64,
    phent: u64,
    phnum: u64,
    interp_base: u64,
    execfn: u64,
) {
    let push = |sp: &mut u64, v: u64| {
        *sp -= 8;
        unsafe { *(*sp as *mut u64) = v; }
    };

    // Terminator
    push(stack_top, AT_NULL); push(stack_top, 0);

    // Random bytes (16 bytes pointer trick: just use stack address)
    let rand_ptr = *stack_top - 16;
    push(stack_top, AT_RANDOM); push(stack_top, rand_ptr);

    push(stack_top, AT_PLATFORM); push(stack_top, b"x86_64\0".as_ptr() as u64);
    push(stack_top, AT_EXECFN);   push(stack_top, execfn);
    push(stack_top, AT_SECURE);   push(stack_top, 0);
    push(stack_top, AT_EGID);     push(stack_top, 0);
    push(stack_top, AT_GID);      push(stack_top, 0);
    push(stack_top, AT_EUID);     push(stack_top, 0);
    push(stack_top, AT_UID);      push(stack_top, 0);
    push(stack_top, AT_CLKTCK);   push(stack_top, 100);
    push(stack_top, AT_HWCAP);    push(stack_top, 0xBFEBFBFF); // typical x86_64
    push(stack_top, AT_HWCAP2);   push(stack_top, 0);
    push(stack_top, AT_FLAGS);    push(stack_top, 0);
    push(stack_top, AT_PAGESZ);   push(stack_top, 4096);
    push(stack_top, AT_BASE);     push(stack_top, interp_base);
    push(stack_top, AT_ENTRY);    push(stack_top, entry);
    push(stack_top, AT_PHNUM);    push(stack_top, phnum);
    push(stack_top, AT_PHENT);    push(stack_top, phent);
    push(stack_top, AT_PHDR);     push(stack_top, phdr);
}

// VDSO page - provides fast clock_gettime etc without syscall overhead
pub const VDSO_VIRT_BASE: u64 = 0x7FFF_F000_0000;

pub fn setup_vdso(space: &mut crate::memory::vmm::AddressSpace) {
    use crate::memory::vmm::Prot;
    // Map a single read-execute page for VDSO
    let _ = space.mmap(
        Some(VDSO_VIRT_BASE),
        4096,
        Prot::READ | Prot::EXEC,
    );
    // TODO: fill page with vdso_clock_gettime trampoline
}

// proc/sys entries that Linux apps commonly probe
pub fn handle_virtual_fs_path(path: &str) -> Option<alloc::vec::Vec<u8>> {
    match path {
        "/proc/sys/kernel/pid_max"      => Some(b"4194304\n".to_vec()),
        "/proc/sys/kernel/hostname"     => Some(b"qunix\n".to_vec()),
        "/proc/sys/kernel/ostype"       => Some(b"Linux\n".to_vec()),
        "/proc/sys/kernel/osrelease"    => Some(b"6.1.0-qunix\n".to_vec()),
        "/proc/sys/kernel/version"      => Some(b"#1 SMP PREEMPT Qunix\n".to_vec()),
        "/proc/sys/kernel/ngroups_max"  => Some(b"65536\n".to_vec()),
        "/proc/sys/vm/overcommit_memory" => Some(b"0\n".to_vec()),
        "/proc/sys/vm/max_map_count"    => Some(b"65530\n".to_vec()),
        "/proc/cpuinfo"                 => Some(build_cpuinfo()),
        "/proc/meminfo"                 => Some(build_meminfo()),
        "/proc/self/maps"               => Some(build_maps()),
        "/proc/self/status"             => Some(build_status()),
        "/proc/self/stat"               => Some(build_stat()),
        "/proc/filesystems"             => Some(b"nodev\ttmpfs\n\text2\n\tfat\n".to_vec()),
        "/etc/hostname"                 => Some(b"qunix\n".to_vec()),
        "/etc/os-release"               => Some(OS_RELEASE.to_vec()),
        "/etc/localtime"                => Some(b"UTC\n".to_vec()),
        _                               => None,
    }
}

const OS_RELEASE: &[u8] = b"\
NAME=\"Qunix\"\n\
VERSION=\"0.2.0\"\n\
ID=qunix\n\
ID_LIKE=linux\n\
PRETTY_NAME=\"Qunix 0.2.0\"\n\
VERSION_ID=\"0.2\"\n\
HOME_URL=\"https://qunix.dev\"\n\
";

fn build_cpuinfo() -> Vec<u8> {
    let (_, _, _, ecx) = crate::arch::x86_64::cpu::cpuid(1);
    alloc::format!(
        "processor\t: 0\nvendor_id\t: QunixCPU\ncpu family\t: 6\n\
         model\t\t: 0\nmodel name\t: Qunix x86_64 CPU\nstepping\t: 0\n\
         cpu MHz\t\t: 3000.000\ncache size\t: 8192 KB\n\
         flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr\n\
         bogomips\t: 6000.00\n\n"
    ).into_bytes()
}

fn build_meminfo() -> Vec<u8> {
    let total_kb = crate::memory::phys::total_frames() * 4;
    let free_kb  = crate::memory::phys::free_frames() * 4;
    alloc::format!(
        "MemTotal:\t{} kB\nMemFree:\t{} kB\nMemAvailable:\t{} kB\n\
         Buffers:\t0 kB\nCached:\t\t0 kB\nSwapTotal:\t0 kB\nSwapFree:\t0 kB\n",
        total_kb, free_kb, free_kb
    ).into_bytes()
}

fn build_maps() -> Vec<u8> {
    let mut s = alloc::string::String::new();
    crate::process::with_current(|p| {
        for r in &p.address_space.regions {
            let prot = {
                let mut ps = alloc::string::String::from("---p");
                if r.prot.contains(crate::memory::vmm::Prot::READ)  { ps.replace_range(0..1, "r"); }
                if r.prot.contains(crate::memory::vmm::Prot::WRITE) { ps.replace_range(1..2, "w"); }
                if r.prot.contains(crate::memory::vmm::Prot::EXEC)  { ps.replace_range(2..3, "x"); }
                ps
            };
            s.push_str(&alloc::format!("{:016x}-{:016x} {} 0 00:00 0\n",
                r.start, r.end, prot));
        }
    });
    s.into_bytes()
}

fn build_status() -> Vec<u8> {
    let (pid, name, uid, gid) = crate::process::with_current(|p| {
        (p.pid, p.name.clone(), p.uid, p.gid)
    }).unwrap_or((0, alloc::string::String::from("unknown"), 0, 0));
    alloc::format!(
        "Name:\t{}\nPid:\t{}\nUid:\t{} {} {} {}\nGid:\t{} {} {} {}\n\
         VmRSS:\t1024 kB\nThreads:\t1\n",
        name, pid, uid, uid, uid, uid, gid, gid, gid, gid
    ).into_bytes()
}

fn build_stat() -> Vec<u8> {
    let pid = crate::process::current_pid();
    alloc::format!("{} (qunix) S 0 {} {} 0 0 0 0 0 0 0 0 0 0 0 0 20 0 1 0 0 0 0\n",
        pid, pid, pid).into_bytes()
}
