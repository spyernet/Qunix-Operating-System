/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

pub mod numa;
pub mod thp;
pub mod zswap;
pub mod heap;
pub mod phys;
pub mod vmm;

use crate::boot::BootInfo;

pub fn init(boot_info: &BootInfo) {
    phys::init(boot_info);
    // NUMA topology from ACPI SRAT
    numa::init_from_srat(boot_info.rsdp_addr);
    numa::populate_nodes();
    crate::klog!("Physical memory: {} MB free",
        phys::free_frames() * 4096 / 1024 / 1024);
    heap::init();
    crate::klog!("Kernel heap initialized");
    vmm::init();
    crate::klog!("VMM initialized");
    zswap::init();
    // zram::zram_init_default();
}
