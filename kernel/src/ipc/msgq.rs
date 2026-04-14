/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! POSIX message queues — mq_open, mq_send, mq_receive.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use crate::process::Pid;

const MAX_MSGS:    usize = 64;
const MAX_MSG_SZ:  usize = 8192;

pub struct Message {
    pub prio: u32,
    pub data: Vec<u8>,
}

pub struct MsgQueue {
    pub name:      String,
    pub msgs:      VecDeque<Message>,
    pub maxmsg:    usize,
    pub msgsize:   usize,
    pub mode:      u32,
    pub refs:      u32,
    pub waiters_r: VecDeque<Pid>,
    pub waiters_w: VecDeque<Pid>,
}

static QUEUES: Mutex<BTreeMap<String, MsgQueue>> = Mutex::new(BTreeMap::new());

pub fn mq_open(name: &str, oflag: i32, mode: u32, maxmsg: usize, msgsize: usize) -> i32 {
    let create = oflag & 0o100 != 0;
    let excl   = oflag & 0o200 != 0;
    let mut qs = QUEUES.lock();

    if qs.contains_key(name) {
        if create && excl { return -17; }
        qs.get_mut(name).unwrap().refs += 1;
        return 0;
    }
    if !create { return -2; }

    qs.insert(String::from(name), MsgQueue {
        name:     String::from(name),
        msgs:     VecDeque::new(),
        maxmsg:   if maxmsg == 0 { MAX_MSGS } else { maxmsg },
        msgsize:  if msgsize == 0 { MAX_MSG_SZ } else { msgsize },
        mode,
        refs: 1,
        waiters_r: VecDeque::new(),
        waiters_w: VecDeque::new(),
    });
    0
}

pub fn mq_close(name: &str) -> i32 {
    let mut qs = QUEUES.lock();
    if let Some(q) = qs.get_mut(name) {
        if q.refs > 0 { q.refs -= 1; }
    }
    0
}

pub fn mq_unlink(name: &str) -> i32 {
    QUEUES.lock().remove(name).map(|_| 0).unwrap_or(-2)
}

pub fn mq_send(name: &str, data: &[u8], prio: u32) -> i32 {
    loop {
        let waiter = {
            let mut qs = QUEUES.lock();
            match qs.get_mut(name) {
                None    => return -2,
                Some(q) => {
                    if q.msgs.len() < q.maxmsg {
                        let n = data.len().min(q.msgsize);
                        q.msgs.push_back(Message { prio, data: data[..n].to_vec() });
                        q.waiters_r.pop_front()
                    } else {
                        let pid = crate::process::current_pid();
                        q.waiters_w.push_back(pid);
                        None
                    }
                }
            }
        };
        if let Some(pid) = waiter { crate::sched::wake_process(pid); return 0; }
        crate::sched::block_current(crate::process::ProcessState::Sleeping);
    }
}

pub fn mq_receive(name: &str, buf: &mut [u8]) -> i32 {
    loop {
        let (result, waiter) = {
            let mut qs = QUEUES.lock();
            match qs.get_mut(name) {
                None    => return -2,
                Some(q) => {
                    if let Some(msg) = q.msgs.pop_front() {
                        let n = msg.data.len().min(buf.len());
                        buf[..n].copy_from_slice(&msg.data[..n]);
                        let w = q.waiters_w.pop_front();
                        (Some(n as i32), w)
                    } else {
                        let pid = crate::process::current_pid();
                        q.waiters_r.push_back(pid);
                        (None, None)
                    }
                }
            }
        };
        if let Some(pid) = waiter { crate::sched::wake_process(pid); }
        if let Some(n) = result { return n; }
        crate::sched::block_current(crate::process::ProcessState::Sleeping);
    }
}
