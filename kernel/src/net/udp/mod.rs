/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use spin::Mutex;
use crate::net::ip::IpHeader;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct UdpHeader {
    pub src_port: u16, pub dst_port: u16,
    pub length:   u16, pub checksum: u16,
}

struct UdpSocket {
    local_port:  u16,
    recv_queue:  Vec<(u32, u16, Vec<u8>)>, // (src_ip, src_port, data)
}

static SOCKETS: Mutex<BTreeMap<u16, UdpSocket>> = Mutex::new(BTreeMap::new());

pub fn bind(local_port: u16) {
    SOCKETS.lock().insert(local_port, UdpSocket { local_port, recv_queue: Vec::new() });
}

pub fn send(src_ip: u32, src_port: u16, dst_ip: u32, dst_port: u16, data: &[u8]) {
    let hlen   = core::mem::size_of::<UdpHeader>();
    let length = (hlen + data.len()) as u16;
    let hdr    = UdpHeader {
        src_port: src_port.to_be(), dst_port: dst_port.to_be(),
        length: length.to_be(), checksum: 0,
    };
    let mut pkt = alloc::vec![0u8; length as usize];
    unsafe { core::ptr::copy_nonoverlapping(&hdr as *const _ as *const u8, pkt.as_mut_ptr(), hlen); }
    pkt[hlen..].copy_from_slice(data);
    crate::net::ip::send(dst_ip, crate::net::ip::PROTO_UDP, &pkt);
}

pub fn receive(ip_hdr: &IpHeader, datagram: &[u8]) {
    if datagram.len() < 8 { return; }
    let hdr      = unsafe { &*(datagram.as_ptr() as *const UdpHeader) };
    let src_ip   = u32::from_be(ip_hdr.src);
    let src_port = u16::from_be(hdr.src_port);
    let dst_port = u16::from_be(hdr.dst_port);
    let payload  = &datagram[8..];

    let mut sockets = SOCKETS.lock();
    if let Some(sock) = sockets.get_mut(&dst_port) {
        sock.recv_queue.push((src_ip, src_port, payload.to_vec()));
    }
    drop(sockets);
    // Also deliver to POSIX socket layer (for userspace recv)
    let dst_ip = u32::from_be(ip_hdr.dst);
    crate::net::socket::udp_deliver(dst_ip, dst_port, src_ip, src_port, payload);
}

pub fn recv(local_port: u16, buf: &mut [u8]) -> Option<(u32, u16, usize)> {
    let mut sockets = SOCKETS.lock();
    if let Some(sock) = sockets.get_mut(&local_port) {
        if let Some((src_ip, src_port, data)) = sock.recv_queue.first().cloned() {
            let n = data.len().min(buf.len());
            buf[..n].copy_from_slice(&data[..n]);
            sock.recv_queue.remove(0);
            return Some((src_ip, src_port, n));
        }
    }
    None
}
