/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

pub mod driver_host;
pub mod acpi;
pub mod block;
pub mod gpu;
pub mod irq;
pub mod keyboard;
pub mod net;
pub mod nvme;
pub mod pcie;
pub mod serial;
pub mod vga;

pub fn init() {
    serial::init();
    vga::init();
    keyboard::init();
    net::init();
    crate::klog!("Core drivers initialized");
}

pub fn init_pci(rsdp: u64) {
    acpi::init(rsdp);
    pcie::init();
    crate::klog!("PCI/ACPI initialized");
}
