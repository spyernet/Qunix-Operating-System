/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! POSIX semaphores — sem_open, sem_wait, sem_post, sem_timedwait.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use spin::Mutex;
use crate::process::Pid;

pub struct Semaphore {
    pub name:    String,
    pub value:   i32,
    pub waiters: VecDeque<Pid>,
    pub mode:    u32,
    pub refs:    u32,
}

static SEMS: Mutex<BTreeMap<String, Semaphore>> = Mutex::new(BTreeMap::new());

pub fn sem_open(name: &str, oflag: i32, mode: u32, value: u32) -> i32 {
    let create  = oflag & 0o100 != 0;
    let excl    = oflag & 0o200 != 0;
    let mut map = SEMS.lock();

    if map.contains_key(name) {
        if create && excl { return -17; } // EEXIST
        map.get_mut(name).unwrap().refs += 1;
        return 0;
    }
    if !create { return -2; } // ENOENT

    map.insert(String::from(name), Semaphore {
        name:    String::from(name),
        value:   value as i32,
        waiters: VecDeque::new(),
        mode,
        refs: 1,
    });
    0
}

pub fn sem_close(name: &str) -> i32 {
    let mut map = SEMS.lock();
    if let Some(s) = map.get_mut(name) {
        if s.refs > 0 { s.refs -= 1; }
    }
    0
}

pub fn sem_unlink(name: &str) -> i32 {
    SEMS.lock().remove(name).map(|_| 0).unwrap_or(-2)
}

pub fn sem_wait(name: &str) -> i32 {
    loop {
        {
            let mut map = SEMS.lock();
            if let Some(s) = map.get_mut(name) {
                if s.value > 0 { s.value -= 1; return 0; }
                let pid = crate::process::current_pid();
                s.waiters.push_back(pid);
            } else { return -2; }
        }
        crate::sched::block_current(crate::process::ProcessState::Sleeping);
    }
}

pub fn sem_trywait(name: &str) -> i32 {
    let mut map = SEMS.lock();
    match map.get_mut(name) {
        Some(s) if s.value > 0 => { s.value -= 1; 0 }
        Some(_)                => -11, // EAGAIN
        None                   => -2,
    }
}

pub fn sem_post(name: &str) -> i32 {
    let waiter = {
        let mut map = SEMS.lock();
        match map.get_mut(name) {
            Some(s) => { s.value += 1; s.waiters.pop_front() }
            None    => return -2,
        }
    };
    if let Some(pid) = waiter { crate::sched::wake_process(pid); }
    0
}

pub fn sem_getvalue(name: &str) -> i32 {
    SEMS.lock().get(name).map(|s| s.value).unwrap_or(-2)
}
