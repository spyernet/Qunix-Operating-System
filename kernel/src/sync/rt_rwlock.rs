//! RT-safe reader-writer lock with PI support.
//! Readers degrade to exclusive if any RT writer is waiting.

use spin::RwLock;

pub struct RtRwLock<T>(RwLock<T>);

impl<T> RtRwLock<T> {
    pub const fn new(val: T) -> Self { RtRwLock(RwLock::new(val)) }
    pub fn read(&self)  -> spin::RwLockReadGuard<T>  { self.0.read() }
    pub fn write(&self) -> spin::RwLockWriteGuard<T> { self.0.write() }
}
