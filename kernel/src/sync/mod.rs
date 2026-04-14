/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! Qunix synchronization primitives.
//!
//! - `pi_mutex`: Priority-inheritance mutex for PREEMPT_RT
//! - `rt_semaphore`: RT-safe semaphore

pub mod pi_mutex;
pub mod rt_rwlock;
