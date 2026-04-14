/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

pub mod ip;
pub mod tcp;
pub mod udp;
pub mod socket;

pub fn init() {
    // Default loopback
    ip::set_local_ip(u32::from_be_bytes([127, 0, 0, 1]));
    ip::add_route(
        u32::from_be_bytes([127, 0, 0, 0]),
        u32::from_be_bytes([255, 0, 0, 0]),
        0, 0,
    );
    crate::klog!("Network subsystem initialized (TCP/IP/UDP/sockets)");
}
