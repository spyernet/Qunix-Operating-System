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

pub const TCP_FIN: u16 = 1 << 0;
pub const TCP_SYN: u16 = 1 << 1;
pub const TCP_RST: u16 = 1 << 2;
pub const TCP_PSH: u16 = 1 << 3;
pub const TCP_ACK: u16 = 1 << 4;

const TCP_MSS:    usize = 1460;
const TCP_WINDOW: u16   = 65535;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum TcpState {
    Closed, Listen, SynSent, SynReceived,
    Established, FinWait1, FinWait2,
    TimeWait, CloseWait, LastAck,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct TcpHeader {
    pub src_port: u16, pub dst_port: u16,
    pub seq: u32,      pub ack: u32,
    pub data_offset_flags: u16,
    pub window: u16,   pub checksum: u16, pub urgent: u16,
}

impl TcpHeader {
    pub fn data_offset(&self) -> usize {
        ((u16::from_be(self.data_offset_flags) >> 12) & 0xF) as usize * 4
    }
    pub fn flags(&self) -> u16 { u16::from_be(self.data_offset_flags) & 0x1FF }
    pub fn has(&self, f: u16) -> bool { self.flags() & f != 0 }
    pub fn new(sp: u16, dp: u16, seq: u32, ack: u32, flags: u16, wnd: u16) -> Self {
        TcpHeader {
            src_port: sp.to_be(), dst_port: dp.to_be(),
            seq: seq.to_be(), ack: ack.to_be(),
            data_offset_flags: ((5u16 << 12) | flags).to_be(),
            window: wnd.to_be(), checksum: 0, urgent: 0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConnKey {
    pub local_ip: u32, pub local_port: u16,
    pub remote_ip: u32, pub remote_port: u16,
}

pub struct TcpSocket {
    pub key:      ConnKey,
    pub state:    TcpState,
    pub snd_una:  u32,
    pub snd_nxt:  u32,
    pub rcv_nxt:  u32,
    pub rcv_wnd:  u16,
    pub send_buf: Vec<u8>,
    pub recv_buf: Vec<u8>,
    pub backlog:  Vec<ConnKey>,
}

impl TcpSocket {
    pub fn new(local_ip: u32, local_port: u16, remote_ip: u32, remote_port: u16) -> Self {
        TcpSocket {
            key: ConnKey { local_ip, local_port, remote_ip, remote_port },
            state: TcpState::Closed,
            snd_una: 0, snd_nxt: 0x1234_0000, rcv_nxt: 0,
            rcv_wnd: TCP_WINDOW,
            send_buf: Vec::new(), recv_buf: Vec::new(), backlog: Vec::new(),
        }
    }
}

static SOCKETS: Mutex<BTreeMap<ConnKey, TcpSocket>> = Mutex::new(BTreeMap::new());

pub fn receive(ip_hdr: &IpHeader, seg: &[u8]) {
    if seg.len() < 20 { return; }
    let hdr      = unsafe { &*(seg.as_ptr() as *const TcpHeader) };
    let src_ip   = u32::from_be(ip_hdr.src);
    let dst_ip   = u32::from_be(ip_hdr.dst);
    let src_port = u16::from_be(hdr.src_port);
    let dst_port = u16::from_be(hdr.dst_port);
    let seq      = u32::from_be(hdr.seq);
    let ack      = u32::from_be(hdr.ack);
    let off      = hdr.data_offset();
    let payload  = if off <= seg.len() { &seg[off..] } else { &[] };

    let conn_key   = ConnKey { local_ip: dst_ip, local_port: dst_port, remote_ip: src_ip, remote_port: src_port };
    let listen_key = ConnKey { local_ip: dst_ip, local_port: dst_port, remote_ip: 0, remote_port: 0 };

    let mut sockets = SOCKETS.lock();

    if let Some(sock) = sockets.get_mut(&conn_key) {
        handle_segment(sock, hdr, seq, ack, payload, src_ip);
        return;
    }
    // SYN to listener
    if hdr.has(TCP_SYN) && !hdr.has(TCP_ACK) && sockets.contains_key(&listen_key) {
        let isn  = 0xDEAD_C0DEu32;
        let mut sock = TcpSocket::new(dst_ip, dst_port, src_ip, src_port);
        sock.state   = TcpState::SynReceived;
        sock.rcv_nxt = seq.wrapping_add(1);
        sock.snd_nxt = isn.wrapping_add(1);
        sock.snd_una = isn;
        drop(sockets);
        send_raw(src_ip, &TcpHeader::new(
            dst_port, src_port, isn, seq.wrapping_add(1), TCP_SYN | TCP_ACK, TCP_WINDOW,
        ), &[]);
        SOCKETS.lock().insert(conn_key, sock);
    }
}

fn handle_segment(sock: &mut TcpSocket, hdr: &TcpHeader, seq: u32, ack: u32, payload: &[u8], src_ip: u32) {
    if hdr.has(TCP_RST) { sock.state = TcpState::Closed; return; }
    match sock.state {
        TcpState::SynSent => {
            if hdr.has(TCP_SYN | TCP_ACK) {
                sock.rcv_nxt = seq.wrapping_add(1);
                sock.snd_una = ack;
                sock.state   = TcpState::Established;
                send_raw(src_ip, &TcpHeader::new(
                    sock.key.local_port, sock.key.remote_port,
                    sock.snd_nxt, sock.rcv_nxt, TCP_ACK, TCP_WINDOW,
                ), &[]);
            }
        }
        TcpState::SynReceived => {
            if hdr.has(TCP_ACK) { sock.state = TcpState::Established; }
        }
        TcpState::Established => {
            if hdr.has(TCP_FIN) {
                sock.rcv_nxt = seq.wrapping_add(1);
                sock.state   = TcpState::CloseWait;
                send_raw(src_ip, &TcpHeader::new(
                    sock.key.local_port, sock.key.remote_port,
                    sock.snd_nxt, sock.rcv_nxt, TCP_ACK, TCP_WINDOW,
                ), &[]);
                return;
            }
            if !payload.is_empty() && seq == sock.rcv_nxt {
                sock.recv_buf.extend_from_slice(payload);
                sock.rcv_nxt = sock.rcv_nxt.wrapping_add(payload.len() as u32);
                send_raw(src_ip, &TcpHeader::new(
                    sock.key.local_port, sock.key.remote_port,
                    sock.snd_nxt, sock.rcv_nxt, TCP_ACK, TCP_WINDOW,
                ), &[]);
            }
            if hdr.has(TCP_ACK) { sock.snd_una = ack; }
        }
        TcpState::FinWait1 if hdr.has(TCP_ACK) => { sock.state = TcpState::FinWait2; }
        TcpState::FinWait2 => {
            if hdr.has(TCP_FIN) {
                sock.rcv_nxt = seq.wrapping_add(1);
                sock.state   = TcpState::TimeWait;
                send_raw(src_ip, &TcpHeader::new(
                    sock.key.local_port, sock.key.remote_port,
                    sock.snd_nxt, sock.rcv_nxt, TCP_ACK, TCP_WINDOW,
                ), &[]);
            }
        }
        TcpState::LastAck if hdr.has(TCP_ACK) => { sock.state = TcpState::Closed; }
        _ => {}
    }
}

pub fn send_data(local_ip: u32, local_port: u16, remote_ip: u32, remote_port: u16, data: &[u8]) -> usize {
    let key = ConnKey { local_ip, local_port, remote_ip, remote_port };
    let mut sockets = SOCKETS.lock();
    let sock = match sockets.get_mut(&key) {
        Some(s) if s.state == TcpState::Established => s,
        _ => return 0,
    };
    let seq = sock.snd_nxt;
    let rcv = sock.rcv_nxt;
    sock.snd_nxt = sock.snd_nxt.wrapping_add(data.len() as u32);
    drop(sockets);

    for (i, chunk) in data.chunks(TCP_MSS).enumerate() {
        let chunk_seq = seq.wrapping_add((i * TCP_MSS) as u32);
        send_raw(remote_ip, &TcpHeader::new(
            local_port, remote_port, chunk_seq, rcv, TCP_PSH | TCP_ACK, TCP_WINDOW,
        ), chunk);
    }
    data.len()
}

pub fn listen(local_ip: u32, local_port: u16) {
    let key = ConnKey { local_ip, local_port, remote_ip: 0, remote_port: 0 };
    let mut sock = TcpSocket::new(local_ip, local_port, 0, 0);
    sock.state = TcpState::Listen;
    SOCKETS.lock().insert(key, sock);
}

pub fn connect(local_ip: u32, local_port: u16, remote_ip: u32, remote_port: u16) {
    let key  = ConnKey { local_ip, local_port, remote_ip, remote_port };
    let mut sock = TcpSocket::new(local_ip, local_port, remote_ip, remote_port);
    sock.state   = TcpState::SynSent;
    let seq      = sock.snd_nxt;
    sock.snd_nxt = seq.wrapping_add(1);
    SOCKETS.lock().insert(key, sock);
    send_raw(remote_ip, &TcpHeader::new(
        local_port, remote_port, seq, 0, TCP_SYN, TCP_WINDOW,
    ), &[]);
}

pub fn close(local_ip: u32, local_port: u16, remote_ip: u32, remote_port: u16) {
    let key = ConnKey { local_ip, local_port, remote_ip, remote_port };
    let mut sockets = SOCKETS.lock();
    if let Some(sock) = sockets.get_mut(&key) {
        if sock.state == TcpState::Established {
            let fin = TcpHeader::new(
                local_port, remote_port, sock.snd_nxt, sock.rcv_nxt, TCP_FIN | TCP_ACK, TCP_WINDOW,
            );
            sock.snd_nxt = sock.snd_nxt.wrapping_add(1);
            sock.state   = TcpState::FinWait1;
            drop(sockets);
            send_raw(remote_ip, &fin, &[]);
            return;
        }
    }
    sockets.remove(&key);
}

fn send_raw(dst_ip: u32, hdr: &TcpHeader, payload: &[u8]) {
    let hlen = core::mem::size_of::<TcpHeader>();
    let mut pkt = alloc::vec![0u8; hlen + payload.len()];
    unsafe { core::ptr::copy_nonoverlapping(hdr as *const _ as *const u8, pkt.as_mut_ptr(), hlen); }
    pkt[hlen..].copy_from_slice(payload);
    let csum = checksum(0, dst_ip, &pkt);
    unsafe { *(pkt.as_mut_ptr().add(16) as *mut u16) = csum; }
    crate::net::ip::send(dst_ip, crate::net::ip::PROTO_TCP, &pkt);
}

fn checksum(src: u32, dst: u32, seg: &[u8]) -> u16 {
    let mut s = 0u32;
    s += (src >> 16) & 0xFFFF; s += src & 0xFFFF;
    s += (dst >> 16) & 0xFFFF; s += dst & 0xFFFF;
    s += 6; s += seg.len() as u32;
    let mut i = 0;
    while i + 1 < seg.len() {
        s += u16::from_be_bytes([seg[i], seg[i+1]]) as u32;
        i += 2;
    }
    if i < seg.len() { s += (seg[i] as u32) << 8; }
    while s >> 16 != 0 { s = (s & 0xFFFF) + (s >> 16); }
    !(s as u16)
}

// ── Functions required by socket layer ───────────────────────────────────

/// Query the state of a TCP connection without mutating it.
pub fn connection_state(
    local_ip: u32, local_port: u16,
    remote_ip: u32, remote_port: u16,
) -> TcpState {
    let key = ConnKey { local_ip, local_port, remote_ip, remote_port };
    SOCKETS.lock().get(&key).map(|s| s.state).unwrap_or(TcpState::Closed)
}

/// Drain up to `len` bytes from a TCP connection's receive buffer.
/// If `peek` is true, the data is not consumed.
/// Returns the number of bytes copied into `buf_ptr`.
pub fn drain_recv(
    local_ip: u32, local_port: u16,
    remote_ip: u32, remote_port: u16,
    buf_ptr: u64, len: usize, peek: bool,
) -> i64 {
    let key = ConnKey { local_ip, local_port, remote_ip, remote_port };
    let mut sockets = SOCKETS.lock();
    let sock = match sockets.get_mut(&key) { Some(s) => s, None => return 0 };
    if sock.recv_buf.is_empty() { return 0; }
    let n = sock.recv_buf.len().min(len);
    unsafe {
        core::ptr::copy_nonoverlapping(sock.recv_buf.as_ptr(), buf_ptr as *mut u8, n);
    }
    if !peek {
        sock.recv_buf.drain(..n);
    }
    n as i64
}

/// Pop a connection that has completed the three-way handshake from
/// the accept queue for a listening port.
/// Returns (remote_ip, remote_port) if a connection is ready.
pub fn pop_accepted(local_ip: u32, local_port: u16) -> Option<(u32, u16)> {
    let mut sockets = SOCKETS.lock();
    // Find a SynReceived that has transitioned to Established against our listen port
    let key = sockets.iter()
        .find(|(k, s)| {
            k.local_port == local_port
            && k.remote_ip != 0
            && s.state == TcpState::Established
            && !ACCEPT_DELIVERED.lock().contains(k)
        })
        .map(|(k, _)| k.clone());

    if let Some(k) = key {
        ACCEPT_DELIVERED.lock().insert(k.clone());
        return Some((k.remote_ip, k.remote_port));
    }
    None
}

// Track which connections have been handed to accept() to avoid double-deliver
use alloc::collections::BTreeSet;
static ACCEPT_DELIVERED: Mutex<BTreeSet<ConnKey>> = Mutex::new(BTreeSet::new());
