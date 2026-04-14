/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

#![no_std]
#![no_main]
#![feature(abi_x86_interrupt, alloc_error_handler, naked_functions, asm_const)]

extern crate alloc;

mod arch;
mod boot;
mod debug;
mod drm;
mod device;
mod drivers;
mod elf;
mod fs;
mod io_uring;
mod ipc;
mod abi_compat;
mod plugins;
mod memory;
mod module;
mod net;
mod perf;
mod process;
mod rtos;
mod sched;
mod sync;
mod security;
mod signal;
mod syscall;
mod time;
mod tty;
mod user;
mod utils;
mod vfs;

use boot::BootInfo;

#[no_mangle]
pub extern "sysv64" fn kernel_main(boot_info_ptr: u64) -> ! {
    let boot_info = unsafe { &*(boot_info_ptr as *const BootInfo) };

    drivers::serial::init();
    klog!("Qunix v0.2.0 booting");

    arch::x86_64::gdt::init();
    arch::x86_64::idt::init();
    arch::x86_64::cpu::init();
    arch::x86_64::smp::bsp_init();

    memory::init(boot_info);
    klog!("Memory: {}MB / {}MB",
        memory::phys::free_frames()  * 4 / 1024,
        memory::phys::total_frames() * 4 / 1024);

    time::init();

    if boot_info.framebuffer_addr != 0 {
        drivers::gpu::init(
            boot_info.framebuffer_addr,
            boot_info.framebuffer_width,
            boot_info.framebuffer_height,
            boot_info.framebuffer_pitch,
            boot_info.framebuffer_format,
        );
        // DRM/KMS subsystem — must be after gpu::init() so dimensions() works
        drm::init(
            boot_info.framebuffer_addr,
            boot_info.framebuffer_width,
            boot_info.framebuffer_height,
            boot_info.framebuffer_pitch,
            boot_info.framebuffer_format,
        );
    }

    device::init();
    drivers::init();
    drivers::init_pci(boot_info.rsdp_addr);

    vfs::init();
    fs::init(boot_info);
    crate::klog!("fs: init returned");

    process::init();
    sched::init();
    crate::klog!("process/sched init returned");

    // syscall::init() // removed - no such fn
    signal::init();
    ipc::init();
    io_uring::init();
    net::init();
    security::init();
    security::namespace::init();
    security::memory_tagging::init();
    module::init();
    abi_compat::init();

    // Register and initialize plugins (compiled from plugins/ directory)
    plugins::generated::register_all();
    plugins::init();
    klog!("Qunix kernel ready — launching userland init");
    user::launch_init();
    loop { unsafe { core::arch::asm!("hlt"); } }
}

#[macro_export]
macro_rules! klog {
    ($($arg:tt)*) => { $crate::debug::_klog(format_args!($($arg)*)) };
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    debug::panic_handler(info)
}

#[alloc_error_handler]
fn alloc_error(layout: core::alloc::Layout) -> ! {
    panic!("Alloc failed: {:?}", layout)
}
