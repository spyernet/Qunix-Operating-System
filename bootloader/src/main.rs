#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec::Vec;
use uefi::prelude::*;
use uefi::Identify;
use uefi::proto::media::fs::SimpleFileSystem;
use uefi::proto::media::file::{File, FileAttribute, FileInfo, FileMode, RegularFile};
use uefi::proto::console::gop::GraphicsOutput;
use uefi::table::boot::{AllocateType, MemoryDescriptor, MemoryType, SearchType};
use uefi::CStr16;

#[repr(C)]
pub struct BootInfo {
    pub memory_map_addr:            u64,
    pub memory_map_count:           u64,
    pub memory_map_descriptor_size: u64,
    pub framebuffer_addr:           u64,
    pub framebuffer_width:          u32,
    pub framebuffer_height:         u32,
    pub framebuffer_pitch:          u32,
    pub framebuffer_format:         u32,
    pub kernel_phys_start:          u64,
    pub kernel_phys_end:            u64,
    pub rsdp_addr:                  u64,
    pub init_phys_start:            u64,
    pub init_size:                  u64,
    pub qshell_phys_start:          u64,
    pub qshell_size:                u64,
}

const PAGE_SIZE: u64 = 4096;
// Kernel higher-half base: must match kernel.ld KERNEL_VIRT_BASE
const KERNEL_VIRT_BASE: u64 = 0xFFFF_8000_0000_0000;
// Map first 4 GB in the higher-half direct map
const DIRECT_MAP_PAGES: u64 = 512; // 512 * 1GB = 512GB worth (use 2MB pages below)

#[entry]
fn efi_main(image: Handle, mut st: SystemTable<Boot>) -> Status {
    uefi_services::init(&mut st).unwrap();
    let bt = st.boot_services();

    uefi_services::println!("Qunix Bootloader v0.2");

    let rsdp    = find_rsdp(&st);
    let fb_info = init_gop(bt);
    let kdata   = read_file(bt, "\\EFI\\QUNIX\\KERNEL.ELF");
    let init    = read_optional_file(bt, "\\EFI\\QUNIX\\INIT.ELF");
    let qshell  = read_optional_file(bt, "\\EFI\\QUNIX\\QSHELL.ELF");

    uefi_services::println!("Kernel: {} bytes", kdata.len());

    let (entry, phys_start, phys_end) = load_elf(bt, &kdata);
    uefi_services::println!("Entry: {:#x}  phys [{:#x}..{:#x}]", entry, phys_start, phys_end);

    let boot_info_phys = bt
        .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
        .expect("boot_info alloc");

    let mmap_size = bt.memory_map_size();
    let mmap_buf_size = mmap_size.map_size + 8 * mmap_size.entry_size;
    let mmap_pages = (mmap_buf_size as u64 + PAGE_SIZE - 1) / PAGE_SIZE;
    let mmap_store = bt
        .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, mmap_pages as usize)
        .expect("mmap alloc");

    let mut mmap_buf: Vec<u8> = alloc::vec![0u8; mmap_buf_size + mmap_size.entry_size];

    let boot_info = unsafe { &mut *(boot_info_phys as *mut BootInfo) };
    boot_info.memory_map_addr            = mmap_store;
    boot_info.memory_map_count           = 0;
    boot_info.memory_map_descriptor_size = mmap_size.entry_size as u64;
    boot_info.framebuffer_addr           = fb_info.0;
    boot_info.framebuffer_width          = fb_info.1;
    boot_info.framebuffer_height         = fb_info.2;
    boot_info.framebuffer_pitch          = fb_info.3;
    boot_info.framebuffer_format         = fb_info.4;
    boot_info.kernel_phys_start          = phys_start;
    boot_info.kernel_phys_end            = phys_end;
    boot_info.rsdp_addr                  = rsdp.unwrap_or(0);
    boot_info.init_phys_start            = 0;
    boot_info.init_size                  = 0;
    boot_info.qshell_phys_start          = 0;
    boot_info.qshell_size                = 0;

    if let Some(data) = init.as_deref() {
        let (phys, size) = load_blob(bt, data);
        boot_info.init_phys_start = phys;
        boot_info.init_size = size;
    }
    if let Some(data) = qshell.as_deref() {
        let (phys, size) = load_blob(bt, data);
        boot_info.qshell_phys_start = phys;
        boot_info.qshell_size = size;
    }

    // Allocate page tables for higher-half mapping before exiting boot services.
    // Layout: 1 PML4 + 1 PDPT + 4 PDs (2MB huge pages), mapping first 4GB
    // both as identity and at KERNEL_VIRT_BASE.
    let pml4_phys = bt
        .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
        .expect("pml4 alloc");
    // 1 PDPT + 4 page directories.
    let pdpt_low_phys = bt
        .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 5)
        .expect("pdpt alloc");

    unsafe {
        build_page_tables(pml4_phys, pdpt_low_phys);
    }

    unsafe {
        let (_runtime_st, mmap_iter) = st
            .exit_boot_services(image, &mut mmap_buf)
            .expect("exit boot services");
        let count = mmap_iter.len() as u64;
        for (i, desc) in mmap_iter.clone().enumerate() {
            let dst = (mmap_store as *mut u8).add(i * mmap_size.entry_size) as *mut MemoryDescriptor;
            core::ptr::write(dst, *desc);
        }
        boot_info.memory_map_count = count;

        // Switch to our page tables that include KERNEL_VIRT_BASE mapping
        core::arch::asm!("mov cr3, {}", in(reg) pml4_phys, options(nomem));

        let kernel_main: extern "sysv64" fn(u64) -> ! = core::mem::transmute(entry);
        kernel_main(boot_info_phys);
    }
}

/// Build page tables:
///   PML4[0] and PML4[256] point to one PDPT.
///   PDPT[0..3] point to four PDs that map 0..4GB with 2MB huge pages.
/// pdpt_base_phys: 5 contiguous pages (PDPT + 4 PD tables).
unsafe fn build_page_tables(pml4_phys: u64, pdpt_base_phys: u64) {
    // Clear PML4
    let pml4 = pml4_phys as *mut u64;
    core::ptr::write_bytes(pml4, 0, 512);

    let pdpt_phys = pdpt_base_phys;
    let pdpt = pdpt_phys as *mut u64;

    for g in 0..4u64 {
        let pd_phys = pdpt_base_phys + (g + 1) * PAGE_SIZE;
        let pd = pd_phys as *mut u64;

        // PDPT entry points to a PD table (Present | Writable).
        *pdpt.add(g as usize) = pd_phys | 0x03;

        // Each PD entry maps a 2MB huge page (Present | Writable | PS).
        for i in 0..512u64 {
            let phys_addr = (g << 30) | (i << 21);
            *pd.add(i as usize) = phys_addr | 0x83;
        }
    }

    // Low identity map root.
    *pml4.add(0) = pdpt_phys | 0x03;

    // Higher-half direct map root.
    // KERNEL_VIRT_BASE = 0xFFFF_8000_0000_0000 -> PML4 index 256.
    let high_pml4_idx = ((KERNEL_VIRT_BASE >> 39) & 0x1FF) as usize;
    *pml4.add(high_pml4_idx) = pdpt_phys | 0x03;
}

fn find_rsdp(st: &SystemTable<Boot>) -> Option<u64> {
    use uefi::table::cfg::{ACPI2_GUID, ACPI_GUID};
    for entry in st.config_table() {
        if entry.guid == ACPI2_GUID || entry.guid == ACPI_GUID {
            return Some(entry.address as u64);
        }
    }
    None
}

fn init_gop(bt: &BootServices) -> (u64, u32, u32, u32, u32) {
    let handles = bt
        .locate_handle_buffer(SearchType::ByProtocol(&GraphicsOutput::GUID))
        .expect("GOP");
    let mut gop = bt
        .open_protocol_exclusive::<GraphicsOutput>(*handles.handles().first().unwrap())
        .expect("GOP open");
    let info   = gop.current_mode_info();
    let (w, h) = info.resolution();
    let fb     = gop.frame_buffer().as_mut_ptr() as u64;
    let fmt    = match info.pixel_format() {
        uefi::proto::console::gop::PixelFormat::Rgb => 0u32,
        uefi::proto::console::gop::PixelFormat::Bgr => 1u32,
        _ => 0u32,
    };
    (fb, w as u32, h as u32, info.stride() as u32 * 4, fmt)
}

fn read_file(bt: &BootServices, path_str: &str) -> Vec<u8> {
    let handles = bt
        .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
        .expect("FS");
    let mut fs = bt
        .open_protocol_exclusive::<SimpleFileSystem>(*handles.handles().first().unwrap())
        .expect("FS open");
    let mut root = fs.open_volume().expect("volume");
    let mut pbuf = [0u16; 64];
    let path = CStr16::from_str_with_buf(path_str, &mut pbuf).unwrap();
    let fh = root
        .open(path, FileMode::Read, FileAttribute::empty())
        .expect("file not found");
    let mut file = unsafe { RegularFile::new(fh) };
    let mut ibuf = [0u8; 512];
    let info = file.get_info::<FileInfo>(&mut ibuf).expect("file info");
    let size = info.file_size() as usize;
    let mut data = alloc::vec![0u8; size];
    let _ = file.read(&mut data).expect("read kernel");
    data
}

fn read_optional_file(bt: &BootServices, path_str: &str) -> Option<Vec<u8>> {
    let handles = bt
        .locate_handle_buffer(SearchType::ByProtocol(&SimpleFileSystem::GUID))
        .ok()?;
    let mut fs = bt
        .open_protocol_exclusive::<SimpleFileSystem>(*handles.handles().first().unwrap())
        .ok()?;
    let mut root = fs.open_volume().ok()?;
    let mut pbuf = [0u16; 64];
    let path = CStr16::from_str_with_buf(path_str, &mut pbuf).ok()?;
    let fh = root.open(path, FileMode::Read, FileAttribute::empty()).ok()?;
    let mut file = unsafe { RegularFile::new(fh) };
    let mut ibuf = [0u8; 512];
    let info = file.get_info::<FileInfo>(&mut ibuf).ok()?;
    let size = info.file_size() as usize;
    let mut data = alloc::vec![0u8; size];
    let _ = file.read(&mut data).ok()?;
    Some(data)
}

fn load_blob(bt: &BootServices, data: &[u8]) -> (u64, u64) {
    let pages = (data.len() as u64 + PAGE_SIZE - 1) / PAGE_SIZE;
    let phys = bt
        .allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages as usize)
        .expect("blob alloc");
    unsafe {
        core::ptr::copy_nonoverlapping(data.as_ptr(), phys as *mut u8, data.len());
        let tail = pages as usize * PAGE_SIZE as usize - data.len();
        if tail > 0 {
            core::ptr::write_bytes((phys as *mut u8).add(data.len()), 0, tail);
        }
    }
    (phys, data.len() as u64)
}

fn load_elf(bt: &BootServices, data: &[u8]) -> (u64, u64, u64) {
    assert!(data.len() >= 64 && &data[0..4] == b"\x7fELF" && data[4] == 2);
    let e_entry   = u64::from_le_bytes(data[24..32].try_into().unwrap());
    let e_phoff   = u64::from_le_bytes(data[32..40].try_into().unwrap()) as usize;
    let e_phentsz = u16::from_le_bytes(data[54..56].try_into().unwrap()) as usize;
    let e_phnum   = u16::from_le_bytes(data[56..58].try_into().unwrap()) as usize;

    let mut load_segments: Vec<(usize, u64, usize, usize)> = Vec::new();
    let mut phys_start = u64::MAX;
    let mut phys_end   = 0u64;

    for i in 0..e_phnum {
        let off = e_phoff + i * e_phentsz;
        let ph  = &data[off..off + e_phentsz];
        let p_type   = u32::from_le_bytes(ph[0..4].try_into().unwrap());
        if p_type != 1 { continue; }
        let p_offset = u64::from_le_bytes(ph[8..16].try_into().unwrap()) as usize;
        let p_paddr  = u64::from_le_bytes(ph[24..32].try_into().unwrap());
        let p_filesz = u64::from_le_bytes(ph[32..40].try_into().unwrap()) as usize;
        let p_memsz  = u64::from_le_bytes(ph[40..48].try_into().unwrap()) as usize;
        if p_memsz == 0 { continue; }
        assert!(p_filesz <= p_memsz);
        assert!(p_offset + p_filesz <= data.len());

        load_segments.push((p_offset, p_paddr, p_filesz, p_memsz));

        if p_paddr < phys_start { phys_start = p_paddr; }
        let end = p_paddr + p_memsz as u64;
        if end > phys_end { phys_end = end; }

    }

    assert!(!load_segments.is_empty());
    load_segments.sort_by_key(|seg| seg.1);

    // Allocate segment pages at fixed physical addresses, but avoid
    // re-allocating overlap when adjacent/unaligned PT_LOAD segments share pages.
    let mut allocated_end = 0u64;
    for &(_, p_paddr, _, p_memsz) in &load_segments {
        let seg_start = p_paddr & !(PAGE_SIZE - 1);
        let end = p_paddr + p_memsz as u64;
        let seg_end = (end + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        let alloc_start = if allocated_end > seg_start { allocated_end } else { seg_start };
        if alloc_start < seg_end {
            let pages = (seg_end - alloc_start) / PAGE_SIZE;
            bt.allocate_pages(
                AllocateType::Address(alloc_start),
                MemoryType::LOADER_DATA,
                pages as usize,
            ).expect("seg alloc");
        }
        if seg_end > allocated_end { allocated_end = seg_end; }
    }

    for (p_offset, p_paddr, p_filesz, p_memsz) in load_segments {
        unsafe {
            let dst = p_paddr as *mut u8;
            core::ptr::copy_nonoverlapping(data.as_ptr().add(p_offset), dst, p_filesz);
            core::ptr::write_bytes(dst.add(p_filesz), 0, p_memsz - p_filesz);
        }
    }
    (e_entry, phys_start, phys_end)
}
