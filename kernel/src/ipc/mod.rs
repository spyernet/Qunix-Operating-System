//! Inter-process communication subsystem.
//!
//! Provides pipes, epoll, shared memory, POSIX message queues, and semaphores.

pub mod epoll;
pub mod msgq;
pub mod pipe;
pub mod sem;
pub mod shm;

pub fn init() {
    crate::klog!("IPC: pipes + epoll + shm + semaphores + message queues");
}
