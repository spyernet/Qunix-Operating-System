/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! ELF64 loader — supports static and dynamic (interpreter) binaries.
//!
//! For static binaries: load PT_LOAD segments, set up stack/auxv.
//! For dynamic binaries (ET_DYN or PT_INTERP present): load interpreter
//! at a random base address, pass AT_BASE, AT_PHDR, etc. so that
//! ld-linux.so.2 can complete the linking.
//!
//! All writes go through physical frames so the new address space need
//! not be active during loading.

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use crate::memory::vmm::{AddressSpace, Prot, RegionKind, VmaRegion};
use crate::arch::x86_64::paging::{PageFlags, PageMapper, PAGE_SIZE, phys_to_virt};
use crate::memory::phys::alloc_frame;
use crate::vfs::FileDescriptor;

// ELF header constants
const ELFMAG:   &[u8] = b"\x7fELF";
const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;
const ET_EXEC:  u16 = 2;
const ET_DYN:   u16 = 3;
const PT_NULL:  u32 = 0;
const PT_LOAD:  u32 = 1;
const PT_DYNAMIC: u32 = 2;
const PT_INTERP: u32 = 3;
const PT_NOTE:  u32 = 4;
const PT_PHDR:  u32 = 6;
const PT_TLS:   u32 = 7;
const PT_GNU_EH_FRAME: u32 = 0x6474e550;
const PT_GNU_STACK:    u32 = 0x6474e551;
const PT_GNU_RELRO:    u32 = 0x6474e552;
const PF_X: u32 = 1;
const PF_W: u32 = 2;
const PF_R: u32 = 4;

const USER_STACK_SIZE: u64 = 8 * 1024 * 1024;   // 8 MB
const USER_STACK_TOP:  u64 = 0x0000_7FFF_FFFF_0000;
const INTERP_BASE:     u64 = 0x0000_7FFF_0000_0000; // dynamic linker load base

pub struct ExecResult {
    pub entry:          u64,
    pub stack_top:      u64,
    pub argc:           u64,
    pub argv_ptr:       u64,
    pub envp_ptr:       u64,
    pub address_space:  AddressSpace,
    pub phdr_addr:      u64,   // user-space address of phdrs
    pub phdr_count:     u64,
    pub phdr_entsize:   u64,
    pub at_base:        u64,   // interpreter load base (0 for static)
    pub tls_addr:       u64,   // PT_TLS virtual address
    pub tls_filesz:     u64,
    pub tls_memsz:      u64,
    pub tls_align:      u64,
}

trait ElfSource {
    fn len(&self) -> u64;
    fn read_exact(&self, offset: u64, buf: &mut [u8]) -> Result<(), u32>;
}

struct SliceSource<'a> {
    data: &'a [u8],
}

impl ElfSource for SliceSource<'_> {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_exact(&self, offset: u64, buf: &mut [u8]) -> Result<(), u32> {
        let start = offset as usize;
        let end = start.checked_add(buf.len()).ok_or(crate::vfs::EINVAL)?;
        if end > self.data.len() {
            return Err(crate::vfs::EINVAL);
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }
}

struct FdSource<'a> {
    fd: &'a FileDescriptor,
}

impl ElfSource for FdSource<'_> {
    fn len(&self) -> u64 {
        self.fd.inode.size
    }

    fn read_exact(&self, offset: u64, buf: &mut [u8]) -> Result<(), u32> {
        match self.fd.inode.ops.read(&self.fd.inode, buf, offset) {
            Ok(n) if n == buf.len() => Ok(()),
            Ok(_) => Err(crate::vfs::EIO),
            Err(e) => Err(e),
        }
    }
}

/// Parse an ELF64 header and return (type, entry, phoff, phentsize, phnum, shoff).
const EM_X86_64: u16 = 0x3E;

fn parse_elf64_header(data: &[u8]) -> Option<(u16, u64, u64, u16, u16, u64)> {
    if data.len() < 64 { return None; }
    // ELF magic
    if &data[0..4] != ELFMAG  { return None; }
    // Must be 64-bit
    if data[4] != ELFCLASS64  { return None; }
    // Must be little-endian
    if data[5] != ELFDATA2LSB { return None; }
    // ELF version must be 1
    if data[6] != 1            { return None; }
    // e_machine must be x86-64
    let e_machine = u16::from_le_bytes(data[18..20].try_into().ok()?);
    if e_machine != EM_X86_64 { return None; }
    // e_version must be 1
    let e_version = u32::from_le_bytes(data[20..24].try_into().ok()?);
    if e_version != 1          { return None; }

    let e_type      = u16::from_le_bytes(data[16..18].try_into().ok()?);
    let e_entry     = u64::from_le_bytes(data[24..32].try_into().ok()?);
    let e_phoff     = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let e_phentsize = u16::from_le_bytes(data[54..56].try_into().ok()?);
    let e_phnum     = u16::from_le_bytes(data[56..58].try_into().ok()?);
    let e_shoff     = u64::from_le_bytes(data[40..48].try_into().ok()?);

    // Sanity: phentsize must be at least 56 bytes for a valid ELF64 phdr
    if e_phnum > 0 && (e_phentsize as usize) < 56 { return None; }
    Some((e_type, e_entry, e_phoff, e_phentsize, e_phnum, e_shoff))
}

#[derive(Default)]
struct PhSegment {
    p_type:   u32,
    p_flags:  u32,
    p_offset: u64,
    p_vaddr:  u64,
    p_paddr:  u64,
    p_filesz: u64,
    p_memsz:  u64,
    p_align:  u64,
}

fn parse_phdr(data: &[u8], off: usize) -> Option<PhSegment> {
    if off + 56 > data.len() { return None; }
    let p = &data[off..];
    Some(PhSegment {
        p_type:   u32::from_le_bytes(p[0..4].try_into().ok()?),
        p_flags:  u32::from_le_bytes(p[4..8].try_into().ok()?),
        p_offset: u64::from_le_bytes(p[8..16].try_into().ok()?),
        p_vaddr:  u64::from_le_bytes(p[16..24].try_into().ok()?),
        p_paddr:  u64::from_le_bytes(p[24..32].try_into().ok()?),
        p_filesz: u64::from_le_bytes(p[32..40].try_into().ok()?),
        p_memsz:  u64::from_le_bytes(p[40..48].try_into().ok()?),
        p_align:  u64::from_le_bytes(p[48..56].try_into().ok()?),
    })
}

fn read_small_vec<S: ElfSource>(source: &S, offset: u64, len: usize) -> Result<Vec<u8>, u32> {
    let end = offset.checked_add(len as u64).ok_or(crate::vfs::EINVAL)?;
    if end > source.len() {
        return Err(crate::vfs::EINVAL);
    }
    let mut buf = vec![0u8; len];
    source.read_exact(offset, &mut buf)?;
    Ok(buf)
}

fn load_phdrs<S: ElfSource>(source: &S, e_phoff: u64, e_phentsize: u16, e_phnum: u16) -> Result<Vec<PhSegment>, u32> {
    let table_bytes = (e_phentsize as usize)
        .checked_mul(e_phnum as usize)
        .ok_or(crate::vfs::EINVAL)?;
    let table_end = e_phoff
        .checked_add(table_bytes as u64)
        .ok_or(crate::vfs::EINVAL)?;
    if table_end > source.len() {
        return Err(crate::vfs::EINVAL);
    }

    let raw = read_small_vec(source, e_phoff, table_bytes)?;
    let mut phdrs = Vec::with_capacity(e_phnum as usize);
    for i in 0..e_phnum as usize {
        let off = i * e_phentsize as usize;
        phdrs.push(parse_phdr(&raw, off).ok_or(crate::vfs::EINVAL)?);
    }
    Ok(phdrs)
}

fn flags_to_prot(flags: u32) -> Prot {
    let mut p = Prot::empty();
    if flags & PF_R != 0 { p |= Prot::READ; }
    if flags & PF_W != 0 { p |= Prot::WRITE; }
    if flags & PF_X != 0 { p |= Prot::EXEC; }
    p
}

fn prot_to_page_flags(prot: Prot) -> PageFlags {
    let mut f = PageFlags::PRESENT | PageFlags::USER;
    if prot.contains(Prot::WRITE) { f |= PageFlags::WRITABLE; }
    if !prot.contains(Prot::EXEC) { f |= PageFlags::NO_EXECUTE; }
    f
}

/// Load ELF segments into an address space.
/// `load_bias` is added to all virtual addresses (0 for static, chosen base for PIE).
/// Returns (adjusted_entry, max_load_end, phdr_user_addr).
fn load_segments<S: ElfSource>(
    source: &S,
    space: &mut AddressSpace,
    load_bias: u64,
    e_phoff: u64, e_phentsize: u16, e_phnum: u16,
    e_entry: u64,
) -> Result<(u64, u64, u64, u64, u64, u64, u64, u64), u32> {
    let mut max_load_end = 0u64;
    let mut phdr_user   = 0u64;
    let mut tls_addr    = 0u64;
    let mut tls_filesz  = 0u64;
    let mut tls_memsz   = 0u64;
    let mut tls_align   = 0u64;

    let phdrs = load_phdrs(source, e_phoff, e_phentsize, e_phnum)?;
    for ph in phdrs {

        match ph.p_type {
            PT_PHDR => {
                phdr_user = ph.p_vaddr + load_bias;
            }
            PT_TLS => {
                tls_addr   = ph.p_vaddr + load_bias;
                tls_filesz = ph.p_filesz;
                tls_memsz  = ph.p_memsz;
                tls_align  = ph.p_align;
            }
            PT_GNU_STACK => {
                // Stack executable flag — we ignore for now (default non-exec)
            }
            PT_LOAD => {
                if ph.p_memsz == 0 { continue; }
                if ph.p_vaddr + ph.p_memsz + load_bias > 0x0000_7FFF_FFFF_FFFF {
                    return Err(crate::vfs::EINVAL);
                }

                let vaddr_page  = (ph.p_vaddr + load_bias) & !(PAGE_SIZE - 1);
                let page_offset = ((ph.p_vaddr + load_bias) - vaddr_page) as usize;
                let total_size  = page_offset as u64 + ph.p_memsz;
                let num_pages   = (total_size + PAGE_SIZE - 1) / PAGE_SIZE;
                let prot        = flags_to_prot(ph.p_flags);
                let page_flags  = prot_to_page_flags(prot);

                let mut mapper = PageMapper::new(space.pml4_phys);
                let mut frames = Vec::with_capacity(num_pages as usize);
                for p in 0..num_pages {
                    let frame = match alloc_frame() {
                        Some(frame) => frame,
                        None => {
                            crate::klog!(
                                "ELF: OOM loading PT_LOAD vaddr={:#x} memsz={} pages={} page={} free={} total={}",
                                ph.p_vaddr + load_bias,
                                ph.p_memsz,
                                num_pages,
                                p,
                                crate::memory::phys::free_frames(),
                                crate::memory::phys::total_frames(),
                            );
                            return Err(crate::vfs::ENOMEM);
                        }
                    };
                    unsafe {
                        core::ptr::write_bytes(phys_to_virt(frame) as *mut u8, 0, PAGE_SIZE as usize);
                        mapper.map_page(vaddr_page + p * PAGE_SIZE, frame, page_flags);
                    }
                    frames.push(frame);
                }

                // Copy file data through physical addresses
                if ph.p_filesz > 0 {
                    let src_end = ph.p_offset
                        .checked_add(ph.p_filesz)
                        .ok_or(crate::vfs::EINVAL)?;
                    if src_end > source.len() {
                        return Err(crate::vfs::EINVAL);
                    }

                    let mut scratch = [0u8; PAGE_SIZE as usize];
                    let mut virt_cursor = ph.p_vaddr + load_bias;
                    let mut copied = 0usize;
                    let to_copy = ph.p_filesz as usize;
                    while copied < to_copy {
                        let pg_idx = ((virt_cursor - vaddr_page) / PAGE_SIZE) as usize;
                        let pg_off = (virt_cursor & (PAGE_SIZE - 1)) as usize;
                        let can = (PAGE_SIZE as usize - pg_off).min(to_copy - copied);
                        source.read_exact(ph.p_offset + copied as u64, &mut scratch[..can])?;
                        let frame_v = phys_to_virt(frames[pg_idx]);
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                scratch.as_ptr(),
                                (frame_v + pg_off as u64) as *mut u8,
                                can,
                            );
                        }
                        copied += can;
                        virt_cursor += can as u64;
                    }
                }

                let seg_end = vaddr_page + num_pages * PAGE_SIZE;
                if seg_end > max_load_end { max_load_end = seg_end; }

                space.regions.push(VmaRegion {
                    start: vaddr_page, end: seg_end, prot,
                    kind: RegionKind::Anonymous, flags: 2,
                    name: String::new(), cow: false,
                });
            }
            _ => {}
        }
    }

    let entry = e_entry + load_bias;
    Ok((entry, max_load_end, phdr_user, tls_addr, tls_filesz, tls_memsz, tls_align, load_bias))
}

fn exec_inner<S: ElfSource>(source: &S, argv: &[String], envp: &[String]) -> Result<ExecResult, u32> {
    crate::klog!("ELF: exec start ({} bytes)", source.len());

    let header = read_small_vec(source, 0, 64)?;
    let (e_type, e_entry, e_phoff, e_phentsize, e_phnum, _) =
        parse_elf64_header(&header).ok_or(crate::vfs::EINVAL)?;

    if e_type != ET_EXEC && e_type != ET_DYN {
        return Err(crate::vfs::EINVAL);
    }
    if e_phentsize < 56 { return Err(crate::vfs::EINVAL); }

    let phdrs = load_phdrs(source, e_phoff, e_phentsize, e_phnum)?;

    // Check for interpreter (dynamic linker)
    let mut interp_path: Option<String> = None;
    for ph in &phdrs {
        if ph.p_type == PT_INTERP && ph.p_filesz > 1 {
            let interp = read_small_vec(source, ph.p_offset, ph.p_filesz as usize)?;
            interp_path = Some(
                String::from_utf8_lossy(&interp[..interp.len().saturating_sub(1)]).into_owned()
            );
            break;
        }
    }

    let mut space = AddressSpace::new_user().ok_or_else(|| {
        crate::klog!(
            "ELF: OOM creating user address space free={} total={}",
            crate::memory::phys::free_frames(),
            crate::memory::phys::total_frames(),
        );
        crate::vfs::ENOMEM
    })?;
    crate::klog!("ELF: user address space created");

    // For PIE (ET_DYN without interpreter at same time), pick a load address
    let load_bias = if e_type == ET_DYN && interp_path.is_none() {
        0x0000_4000_0000_0000u64  // base for PIE executables
    } else if e_type == ET_DYN {
        0x0000_5000_0000_0000u64  // base when interpreter is also loaded
    } else {
        0u64  // static executable: load at its own VAs
    };

    // Load main executable
    let (entry, max_load_end, phdr_user, tls_addr, tls_filesz, tls_memsz, tls_align, _) =
        load_segments(source, &mut space, load_bias,
                      e_phoff, e_phentsize, e_phnum, e_entry)?;
    crate::klog!("ELF: main image loaded entry={:#x} brk={:#x}", entry, max_load_end);

    // Load interpreter if present
    let (at_base, final_entry) = if let Some(ref ipath) = interp_path {
        let cwd = crate::process::with_current(|p| p.get_cwd())
            .unwrap_or_else(|| alloc::string::String::from("/"));
        let interp_fd = crate::vfs::open(&cwd, ipath, crate::vfs::O_RDONLY, 0)
            .map_err(|_| crate::vfs::ENOENT)?;
        let interp_source = FdSource { fd: &interp_fd };
        if interp_source.len() < 64 {
            return Err(crate::vfs::ENOENT);
        }

        let interp_header = read_small_vec(&interp_source, 0, 64)?;
        let (_it, ie, iphoff, iphentsz, iphnum, _) =
            parse_elf64_header(&interp_header).ok_or(crate::vfs::EINVAL)?;

        let (interp_entry, _, _, _, _, _, _, _) =
            load_segments(&interp_source, &mut space, INTERP_BASE,
                          iphoff, iphentsz, iphnum, ie)?;

        (INTERP_BASE, interp_entry)
    } else {
        (0, entry)
    };
    crate::klog!("ELF: interpreter stage complete entry={:#x} base={:#x}", final_entry, at_base);

    // Allocate user stack
    let stack_bottom = USER_STACK_TOP - USER_STACK_SIZE;
    let stack_pages  = USER_STACK_SIZE / PAGE_SIZE;
    let stack_flags  = PageFlags::PRESENT | PageFlags::USER | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    let mut mapper   = PageMapper::new(space.pml4_phys);
    let mut stack_frames = Vec::with_capacity(stack_pages as usize);
    for p in 0..stack_pages {
        let frame = match alloc_frame() {
            Some(frame) => frame,
            None => {
                crate::klog!(
                    "ELF: OOM allocating user stack page={} free={} total={}",
                    p,
                    crate::memory::phys::free_frames(),
                    crate::memory::phys::total_frames(),
                );
                return Err(crate::vfs::ENOMEM);
            }
        };
        unsafe {
            core::ptr::write_bytes(phys_to_virt(frame) as *mut u8, 0, PAGE_SIZE as usize);
            mapper.map_page(stack_bottom + p * PAGE_SIZE, frame, stack_flags);
        }
        stack_frames.push(frame);
    }
    space.regions.push(VmaRegion {
        start: stack_bottom, end: USER_STACK_TOP,
        prot: Prot::READ | Prot::WRITE, kind: RegionKind::Stack,
        flags: 2, name: String::from("[stack]"), cow: false,
    });

    let stack_top = setup_stack(
        USER_STACK_TOP, stack_bottom, &stack_frames,
        &argv, &envp,
        final_entry, e_entry + load_bias,
        phdr_user, e_phentsize as u64, e_phnum as u64,
        at_base, tls_addr,
    );
    let argc = argv.len() as u64;
    let argv_ptr = stack_top + 8;
    let envp_ptr = argv_ptr + (argc + 1) * 8;
    crate::klog!("ELF: user stack ready rsp={:#x}", stack_top);

    space.brk       = max_load_end;
    space.brk_start = max_load_end;
    // mmap_base must be above all loaded segments AND above the
    // canonical MMAP_BASE so glibc/ld.so find a consistent gap.
    // Use the higher of MMAP_BASE or (max_load_end + 1 page), page-aligned.
    {
        const MMAP_BASE: u64 = 0x0000_4000_0000_0000;
        let above_segs = (max_load_end + PAGE_SIZE + PAGE_SIZE - 1)
                         & !(PAGE_SIZE - 1);
        space.mmap_base = above_segs.max(MMAP_BASE);
    }
    crate::klog!("ELF: exec result ready mmap_base={:#x}", space.mmap_base);

    Ok(ExecResult {
        entry: final_entry,
        stack_top,
        argc,
        argv_ptr,
        envp_ptr,
        address_space: space,
        phdr_addr:    phdr_user,
        phdr_count:   e_phnum as u64,
        phdr_entsize: e_phentsize as u64,
        at_base,
        tls_addr, tls_filesz, tls_memsz, tls_align,
    })
}

/// Main ELF exec entry point.
pub fn exec(data: Vec<u8>, argv: Vec<String>, envp: Vec<String>) -> Result<ExecResult, u32> {
    let source = SliceSource { data: &data };
    let result = exec_inner(&source, &argv, &envp)?;

    // The current kernel heap path is still fragile on larger frees/reallocations
    // during early boot. Keep the raw ELF image alive for now so exec can hand
    // control to userspace instead of dying during drop.
    core::mem::forget(data);

    Ok(result)
}

pub fn exec_fd(fd: &FileDescriptor, argv: Vec<String>, envp: Vec<String>) -> Result<ExecResult, u32> {
    let source = FdSource { fd };
    exec_inner(&source, &argv, &envp)
}

fn write_phys(frames: &[u64], stack_base: u64, vaddr: u64, val: u64) {
    let pg_idx = ((vaddr - stack_base) / PAGE_SIZE) as usize;
    let pg_off = (vaddr & (PAGE_SIZE - 1)) as usize;
    if pg_idx < frames.len() {
        unsafe {
            let ptr = (phys_to_virt(frames[pg_idx]) + pg_off as u64) as *mut u64;
            *ptr = val;
        }
    }
}

fn write_bytes_phys(frames: &[u64], stack_base: u64, mut vaddr: u64, data: &[u8]) {
    for (i, &b) in data.iter().enumerate() {
        let pg_idx = ((vaddr - stack_base) / PAGE_SIZE) as usize;
        let pg_off = (vaddr & (PAGE_SIZE - 1)) as usize;
        if pg_idx < frames.len() {
            unsafe { *((phys_to_virt(frames[pg_idx]) + pg_off as u64) as *mut u8) = b; }
        }
        vaddr += 1;
    }
}

fn setup_stack(
    stack_top: u64, stack_base: u64, frames: &[u64],
    argv: &[String], envp: &[String],
    entry: u64, at_entry: u64,
    phdr_addr: u64, phent: u64, phnum: u64,
    at_base: u64, tls_addr: u64,
) -> u64 {
    // Build the initial process stack per the System V AMD64 ABI.
    // Stack grows downward. Final layout (RSP points at argc):
    //
    //  high addr ┐
    //  [strings, platform, execfn, random bytes]
    //  auxv pairs: (AT_EXECFN,ptr) ... (AT_NULL,0)
    //  NULL       ← end of envp
    //  envp[n-1]
    //  ...
    //  envp[0]
    //  NULL       ← end of argv
    //  argv[argc-1]
    //  ...
    //  argv[0]
    //  argc       ← RSP here
    //  low addr  ┘

    let mut sp = stack_top;

    // Helper: push a NUL-terminated string onto the stack, return its address.
    // Strings are NOT 16-byte aligned — they pack tightly.
    let push_str = |sp: &mut u64, s: &str| -> u64 {
        let b = s.as_bytes();
        *sp -= b.len() as u64 + 1;  // +1 for NUL
        write_bytes_phys(frames, stack_base, *sp, b);
        write_bytes_phys(frames, stack_base, *sp + b.len() as u64, &[0u8]);
        *sp
    };

    let push_u64 = |sp: &mut u64, v: u64| {
        *sp -= 8;
        write_phys(frames, stack_base, *sp, v);
    };

    // ── 1. Write strings (from top of stack down) ──────────────────────
    // execfn string
    let execfn_str  = argv.first().map(|s| s.as_str()).unwrap_or("/init");
    let execfn_addr = push_str(&mut sp, execfn_str);

    // Platform string
    sp -= 7;   // "x86_64" + NUL = 7 bytes
    sp &= !0xFu64;
    write_bytes_phys(frames, stack_base, sp, b"x86_64 ");
    let platform_addr = sp;

    // AT_RANDOM: 16 bytes of pseudo-random data
    sp -= 16;
    sp &= !0xFu64;
    let rand_addr = sp;
    let tsc = unsafe { crate::arch::x86_64::msr::read(crate::arch::x86_64::msr::IA32_TSC) };
    write_phys(frames, stack_base, sp,      tsc ^ 0xDEAD_BEEF_0102_0304);
    write_phys(frames, stack_base, sp + 8,  tsc ^ 0xBEEF_CAFE_0506_0708);

    // env strings
    let env_ptrs: Vec<u64> = envp.iter().rev().map(|s| push_str(&mut sp, s)).collect::<Vec<_>>().into_iter().rev().collect();

    // arg strings
    let arg_ptrs: Vec<u64> = argv.iter().rev().map(|s| push_str(&mut sp, s)).collect::<Vec<_>>().into_iter().rev().collect();

    // ── 2. Align SP to 16 bytes before the pointer/auxv area ──────────
    sp &= !0xFu64;

    // ── 3. Push auxv pairs ───────────────────────────────────────────
    //
    // Stack grows DOWN; libc walks UP from where it finds argv/envp.
    // Layout after this section (addresses ascending):
    //   [AT_NULL, 0]   ← lowest address, pushed LAST
    //   [AT_...  val]  ← middle entries
    //   [AT_EXECFN,p]  ← highest auxv address, pushed FIRST
    //
    // Rule: push each entry value then key (so key ends up lower in memory
    // than value — correct for the {AT_TYPE, AT_VALUE} pair layout).
    // Push AT_NULL LAST so it terminates the vector at the lowest address.

    use crate::abi_compat::abi::*;
    macro_rules! aux {
        ($k:expr, $v:expr) => {
            push_u64(&mut sp, $v);  // value pushed first → higher address
            push_u64(&mut sp, $k);  // key pushed second  → lower address
        }
    }

    // Push entries in reverse order of reading priority (first-pushed = highest addr).
    // libc will read from low→high, hitting AT_NULL last.
    aux!(AT_EXECFN,   execfn_addr);
    aux!(AT_PLATFORM, platform_addr);
    aux!(AT_RANDOM,   rand_addr);
    aux!(AT_SECURE,   0u64);
    aux!(AT_EGID,     0u64);
    aux!(AT_GID,      0u64);
    aux!(AT_EUID,     0u64);
    aux!(AT_UID,      0u64);
    aux!(AT_ENTRY,    at_entry);
    aux!(AT_FLAGS,    0u64);
    aux!(AT_BASE,     at_base);
    aux!(AT_PHNUM,    phnum);
    aux!(AT_PHENT,    phent);
    aux!(AT_PHDR,     phdr_addr);
    aux!(AT_CLKTCK,   100u64);
    aux!(AT_PAGESZ,   4096u64);
    aux!(AT_HWCAP,    0x078bfbfdu64);
    aux!(AT_HWCAP2,   0u64);
    // AT_NULL terminator — pushed LAST, ends up at lowest address ✓
    push_u64(&mut sp, 0u64);    // AT_NULL value
    push_u64(&mut sp, AT_NULL); // AT_NULL key = 0

    // ── 4. Final alignment before the pointer + argc area ─────────────
    //
    // SysV AMD64 ABI §3.4.1: "The end of the input argument area shall
    // be aligned on a 16-byte boundary."  In practice, crt/_start code is
    // generally entered with %rsp % 16 == 8, matching normal function-entry
    // alignment after a CALL pushed an 8-byte return address. Rust's no_std
    // _start for our user programs is codegen'd with that expectation.
    //
    // After pushing argc/argv/envp/auxv we therefore need sp % 16 == 8 at argc.
    // Words we are about to push (from high addr down):
    //   NULL(envp) + n_envp + NULL(argv) + n_argv + argc  = n_envp+n_argv+3
    // Starting from a 16-byte-aligned SP, an even number of pushed words
    // leaves argc at % 16 == 0 while an odd number leaves it at % 16 == 8.
    // Insert one padding word only when we need to flip the parity.
    sp &= !0xFu64;  // align before counting
    let words_to_push = 1 /*argc*/ + argv.len() + 1 /*NULL*/ + envp.len() + 1 /*NULL*/;
    if words_to_push % 2 == 0 {
        push_u64(&mut sp, 0); // alignment gap
    }

    // ── 5. Push NULL-terminated envp pointer array ─────────────────────
    push_u64(&mut sp, 0); // NULL terminator after envp
    for &ptr in env_ptrs.iter().rev() { push_u64(&mut sp, ptr); }

    // ── 6. Push NULL-terminated argv pointer array ─────────────────────
    push_u64(&mut sp, 0); // NULL terminator after argv
    for &ptr in arg_ptrs.iter().rev() { push_u64(&mut sp, ptr); }

    // ── 7. Push argc ───────────────────────────────────────────────────
    push_u64(&mut sp, argv.len() as u64);

    // sp is now the initial user RSP; user _start expects function-entry alignment.
    debug_assert!(sp % 16 == 8, "stack misaligned at entry: sp={:#x}", sp);
    sp
}

pub unsafe fn exec_usermode_noreturn(entry: u64, stack: u64, argc: u64, argv: u64, envp: u64) -> ! {
    use crate::arch::x86_64::gdt::{USER_CODE_SEL, USER_DATA_SEL};
    use crate::arch::x86_64::msr::{self, IA32_GSBASE, IA32_KERNEL_GSBASE};

    // The initial boot path keeps both GS MSRs pointing at percpu state.
    // execve enters here from syscall context, where swapgs may have left
    // IA32_KERNEL_GSBASE holding the prior user GS value. The rest of the
    // kernel's interrupt/syscall entry paths assume the two are in sync.
    let kernel_gs = msr::read(IA32_GSBASE);
    msr::write(IA32_GSBASE, kernel_gs);
    msr::write(IA32_KERNEL_GSBASE, kernel_gs);

    core::arch::asm!(
        "mov ax, {uds}",
        "mov ds, ax", "mov es, ax",
        "push {uds}",
        "push r8",
        "pushfq",
        "pop  rax",
        "or   rax, 0x200",
        "push rax",
        "push {ucs}",
        "push r9",
        "iretq",
        uds = const USER_DATA_SEL as u64,
        ucs = const USER_CODE_SEL as u64,
        in("rdi") argc,
        in("rsi") argv,
        in("rdx") envp,
        in("r8") stack,
        in("r9") entry,
        options(noreturn)
    );
}
