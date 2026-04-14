/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

#[repr(C)]
pub struct BootInfo {
    pub memory_map_addr: u64,
    pub memory_map_count: u64,
    pub memory_map_descriptor_size: u64,
    pub framebuffer_addr: u64,
    pub framebuffer_width: u32,
    pub framebuffer_height: u32,
    pub framebuffer_pitch: u32,
    pub framebuffer_format: u32,
    pub kernel_phys_start: u64,
    pub kernel_phys_end: u64,
    pub rsdp_addr: u64,
    pub init_phys_start: u64,
    pub init_size: u64,
    pub qshell_phys_start: u64,
    pub qshell_size: u64,
}

#[repr(C)]
pub struct UefiMemoryDescriptor {
    pub mem_type: u32,
    pub _pad: u32,
    pub phys_start: u64,
    pub virt_start: u64,
    pub num_pages: u64,
    pub attribute: u64,
}

pub mod uefi_memory_type {
    pub const CONVENTIONAL: u32 = 7;
    pub const BOOT_SERVICES_CODE: u32 = 3;
    pub const BOOT_SERVICES_DATA: u32 = 4;
    pub const LOADER_CODE: u32 = 1;
    pub const LOADER_DATA: u32 = 2;
    pub const ACPI_RECLAIM: u32 = 9;
    pub const RESERVED: u32 = 0;
}
