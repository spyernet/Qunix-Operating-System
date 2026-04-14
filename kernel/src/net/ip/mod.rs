/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::vec::Vec;
use spin::Mutex;

pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP:  u8 = 6;
pub const PROTO_UDP:  u8 = 17;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct IpHeader {
    pub version_ihl: u8,
    pub dscp_ecn:    u8,
    pub total_len:   u16,
    pub id:          u16,
    pub frag_off:    u16,
    pub ttl:         u8,
    pub protocol:    u8,
    pub checksum:    u16,
    pub src:         u32,
    pub dst:         u32,
}

impl IpHeader {
    pub fn version(&self) -> u8 { self.version_ihl >> 4 }
    pub fn ihl(&self) -> usize  { ((self.version_ihl & 0xF) * 4) as usize }

    pub fn new(src: u32, dst: u32, proto: u8, payload_len: u16) -> Self {
        let total = payload_len + 20;
        let mut h = IpHeader {
            version_ihl: 0x45, dscp_ecn: 0,
            total_len: total.to_be(), id: 0, frag_off: 0,
            ttl: 64, protocol: proto, checksum: 0,
            src: src.to_be(), dst: dst.to_be(),
        };
        h.checksum = ip_checksum(&h);
        h
    }
}

pub fn ip_checksum(hdr: &IpHeader) -> u16 {
    let bytes = unsafe {
        core::slice::from_raw_parts(hdr as *const IpHeader as *const u16, 10)
    };
    let mut sum = 0u32;
    for &w in bytes { sum += u16::from_be(w) as u32; }
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    !(sum as u16)
}

// Route table entry
struct Route {
    network: u32,
    netmask: u32,
    gateway: u32,
    iface:   u32,  // interface index
}

struct NetState {
    local_ip:  u32,
    gateway:   u32,
    routes:    Vec<Route>,
}

static NET: Mutex<NetState> = Mutex::new(NetState {
    local_ip: 0,
    gateway:  0,
    routes:   Vec::new(),
});

pub fn set_local_ip(ip: u32) { NET.lock().local_ip = ip; }
pub fn local_ip() -> u32    { NET.lock().local_ip }
pub fn set_gateway(gw: u32) { NET.lock().gateway = gw; }

pub fn add_route(network: u32, netmask: u32, gateway: u32, iface: u32) {
    NET.lock().routes.push(Route { network, netmask, gateway, iface });
}

pub fn route_lookup(dst_ip: u32) -> Option<u32> {
    let net = NET.lock();
    // Longest prefix match
    let mut best: Option<(u32, u32)> = None;
    for r in &net.routes {
        if dst_ip & r.netmask == r.network {
            match best {
                None => best = Some((r.netmask, r.gateway)),
                Some((m, _)) if r.netmask > m => best = Some((r.netmask, r.gateway)),
                _ => {}
            }
        }
    }
    best.map(|(_, gw)| gw).or(Some(net.gateway))
}

pub fn receive(packet: &[u8]) {
    if packet.len() < 20 { return; }
    let hdr     = unsafe { &*(packet.as_ptr() as *const IpHeader) };
    if hdr.version() != 4 { return; }
    let ihl     = hdr.ihl();
    if ihl > packet.len() { return; }
    let payload = &packet[ihl..];
    match hdr.protocol {
        PROTO_TCP  => crate::net::tcp::receive(hdr, payload),
        PROTO_UDP  => crate::net::udp::receive(hdr, payload),
        PROTO_ICMP => icmp_receive(hdr, payload),
        _          => {}
    }
}

pub fn send(dst_ip: u32, proto: u8, payload: &[u8]) {
    let src_ip  = local_ip();
    let hdr     = IpHeader::new(src_ip, dst_ip, proto, payload.len() as u16);
    let hlen    = 20;
    let mut pkt = alloc::vec![0u8; hlen + payload.len()];
    unsafe { core::ptr::copy_nonoverlapping(&hdr as *const _ as *const u8, pkt.as_mut_ptr(), hlen); }
    pkt[hlen..].copy_from_slice(payload);
    // Hand to NIC driver
    crate::drivers::net::transmit(&pkt);
}

fn icmp_receive(ip_hdr: &IpHeader, payload: &[u8]) {
    if payload.len() < 8 { return; }
    let icmp_type = payload[0];
    if icmp_type == 8 {
        // Echo request — send reply
        let mut reply = payload.to_vec();
        reply[0] = 0; // echo reply
        reply[2] = 0; reply[3] = 0; // zero checksum
        let csum = icmp_checksum(&reply);
        reply[2] = (csum >> 8) as u8;
        reply[3] = (csum & 0xFF) as u8;
        send(u32::from_be(ip_hdr.src), PROTO_ICMP, &reply);
    }
}

fn icmp_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i   = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i+1]]) as u32;
        i += 2;
    }
    if i < data.len() { sum += (data[i] as u32) << 8; }
    while sum >> 16 != 0 { sum = (sum & 0xFFFF) + (sum >> 16); }
    !(sum as u16)
}
