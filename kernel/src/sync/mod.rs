//! Qunix synchronization primitives.
//!
//! - `pi_mutex`: Priority-inheritance mutex for PREEMPT_RT
//! - `rt_semaphore`: RT-safe semaphore

pub mod pi_mutex;
pub mod rt_rwlock;
