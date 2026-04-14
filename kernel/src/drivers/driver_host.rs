/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::string::ToString;
// Driver isolation host — the boundary between the Qunix kernel core and
// loadable device drivers (including Linux-compatible drivers).
//
// Philosophy:
//   - The kernel core (MIT/BSD licensed) never directly calls GPL driver code.
//   - Drivers register themselves via `register_driver()` using a stable ABI.
//   - The host dispatches I/O through the DriverOps trait.
//   - GPL drivers live in their own binary, loaded as kernel modules.
//   - This file is the ONLY crossing point; it is MIT licensed.
//
// Driver types supported:
//   - Block drivers (NVMe, AHCI, virtio-blk)
//   - Network drivers (virtio-net, e1000, rtl8169)
//   - Character drivers (/dev/null, /dev/zero, ttys)
//   - Platform drivers (APIC, timer, PCI host)
//
// Linux driver compatibility:
//   A Linux driver can be loaded if it is compiled for Qunix's driver ABI.
//   It sees a minimal set of Linux kernel APIs (kmalloc, kfree, pci_read_config_*,
//   ioremap, copy_to_user, etc.) provided by the compat layer below.
//   The GPL license applies only to the driver module, not to this host.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use spin::Mutex;

// ── Driver operation tables ───────────────────────────────────────────────

/// Block driver interface — stable ABI.
pub trait BlockDriverOps: Send + Sync {
    /// Return block size in bytes (usually 512 or 4096).
    fn block_size(&self) -> u32;
    /// Return total number of logical blocks.
    fn num_blocks(&self) -> u64;
    /// Synchronous read: fill `buf` starting at logical block `lba`.
    /// `buf.len()` must be a multiple of `block_size()`.
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DriverError>;
    /// Synchronous write.
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DriverError>;
    /// Flush any in-flight writes to stable storage.
    fn flush(&self) -> Result<(), DriverError> { Ok(()) }
    /// Human-readable device name for logging.
    fn name(&self) -> &str;
}

/// Network driver interface — stable ABI.
pub trait NetDriverOps: Send + Sync {
    /// Return MAC address (6 bytes).
    fn mac_address(&self) -> [u8; 6];
    /// Transmit a raw Ethernet frame. Driver owns the copy.
    fn transmit(&self, frame: &[u8]) -> Result<(), DriverError>;
    /// Poll for received frames; call `deliver` for each complete frame.
    fn poll_recv(&self, deliver: &mut dyn FnMut(&[u8]));
    /// Maximum transmission unit in bytes.
    fn mtu(&self) -> u32 { 1500 }
    fn name(&self) -> &str;
}

/// Character / misc driver interface.
pub trait CharDriverOps: Send + Sync {
    fn read(&self, buf: &mut [u8]) -> Result<usize, DriverError>;
    fn write(&self, buf: &[u8]) -> Result<usize, DriverError>;
    fn ioctl(&self, request: u64, arg: u64) -> Result<i64, DriverError> { Ok(0) }
    fn name(&self) -> &str;
}

#[derive(Debug, Clone)]
pub enum DriverError {
    Timeout,
    IoError(u32),
    InvalidArgument,
    NotSupported,
    NoMemory,
    DeviceRemoved,
}

impl core::fmt::Display for DriverError {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        match self {
            DriverError::Timeout          => write!(f, "device timeout"),
            DriverError::IoError(code)    => write!(f, "I/O error {}", code),
            DriverError::InvalidArgument  => write!(f, "invalid argument"),
            DriverError::NotSupported     => write!(f, "not supported"),
            DriverError::NoMemory         => write!(f, "out of memory"),
            DriverError::DeviceRemoved    => write!(f, "device removed"),
        }
    }
}

// ── Driver registry ───────────────────────────────────────────────────────

#[derive(Clone)]
pub struct BlockDevice {
    pub id:     u32,
    pub name:   String,
    pub ops:    Arc<dyn BlockDriverOps>,
}

#[derive(Clone)]
pub struct NetDevice {
    pub id:   u32,
    pub name: String,
    pub ops:  Arc<dyn NetDriverOps>,
}

struct DriverRegistry {
    block_devs: BTreeMap<u32, BlockDevice>,
    net_devs:   BTreeMap<u32, NetDevice>,
    next_block_id: u32,
    next_net_id:   u32,
}

impl DriverRegistry {
    const fn new() -> Self {
        DriverRegistry {
            block_devs: BTreeMap::new(),
            net_devs:   BTreeMap::new(),
            next_block_id: 0,
            next_net_id:   0,
        }
    }
}

static REGISTRY: Mutex<DriverRegistry> = Mutex::new(DriverRegistry::new());

/// Register a block device driver. Returns the assigned device ID.
pub fn register_block(ops: Arc<dyn BlockDriverOps>) -> u32 {
    let mut reg = REGISTRY.lock();
    let id = reg.next_block_id;
    reg.next_block_id += 1;
    let name = ops.name().to_string();
    crate::klog!("driver_host: block device {} registered as block{} ({} blocks of {}B)",
        name, id, ops.num_blocks(), ops.block_size());
    reg.block_devs.insert(id, BlockDevice { id, name, ops });
    id
}

/// Register a network device driver. Returns the assigned device ID.
pub fn register_net(ops: Arc<dyn NetDriverOps>) -> u32 {
    let mut reg = REGISTRY.lock();
    let id = reg.next_net_id;
    reg.next_net_id += 1;
    let mac  = ops.mac_address();
    let name = ops.name().to_string();
    crate::klog!("driver_host: net device {} registered as eth{} MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        name, id, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
    reg.net_devs.insert(id, NetDevice { id, name, ops });
    id
}

/// Look up a block device by ID.
pub fn get_block(id: u32) -> Option<Arc<dyn BlockDriverOps>> {
    REGISTRY.lock().block_devs.get(&id).map(|d| d.ops.clone())
}

/// Look up a net device by ID.
pub fn get_net(id: u32) -> Option<Arc<dyn NetDriverOps>> {
    REGISTRY.lock().net_devs.get(&id).map(|d| d.ops.clone())
}

/// Number of registered block devices.
pub fn block_device_count() -> u32 {
    REGISTRY.lock().next_block_id
}

/// Number of registered net devices.
pub fn net_device_count() -> u32 {
    REGISTRY.lock().next_net_id
}

/// Dispatch block I/O — kernel core uses this instead of calling drivers directly.
pub fn block_read(dev_id: u32, lba: u64, buf: &mut [u8]) -> Result<(), DriverError> {
    let dev = REGISTRY.lock().block_devs.get(&dev_id).map(|d| d.ops.clone());
    match dev {
        Some(ops) => ops.read(lba, buf),
        None      => Err(DriverError::IoError(19)), // ENODEV
    }
}

pub fn block_write(dev_id: u32, lba: u64, buf: &[u8]) -> Result<(), DriverError> {
    let dev = REGISTRY.lock().block_devs.get(&dev_id).map(|d| d.ops.clone());
    match dev {
        Some(ops) => ops.write(lba, buf),
        None      => Err(DriverError::IoError(19)),
    }
}

pub fn net_transmit(dev_id: u32, frame: &[u8]) -> Result<(), DriverError> {
    let dev = REGISTRY.lock().net_devs.get(&dev_id).map(|d| d.ops.clone());
    match dev {
        Some(ops) => ops.transmit(frame),
        None      => Err(DriverError::IoError(19)),
    }
}

pub fn net_poll(dev_id: u32, deliver: &mut dyn FnMut(&[u8])) {
    let dev = REGISTRY.lock().net_devs.get(&dev_id).map(|d| d.ops.clone());
    if let Some(ops) = dev { ops.poll_recv(deliver); }
}

// ── Linux driver compatibility surface ────────────────────────────────────
//
// A driver module compiled for Qunix may call these C-linkage functions.
// They provide stable access to Qunix kernel services without GPL infection.
// ONLY these symbols are exported to drivers. No internal kernel structures
// are shared. This is the isolation boundary.
//
// The driver host boundary is MIT-licensed.
// GPL-licensed driver modules may call it; the license does not propagate.

/// malloc-compatible allocator for driver use.
#[no_mangle]
pub extern "C" fn qunix_kmalloc(size: usize, _flags: u32) -> *mut u8 {
    use alloc::alloc::{alloc, Layout};
    if size == 0 { return core::ptr::null_mut(); }
    let layout = match Layout::from_size_align(size, 8) {
        Ok(l)  => l,
        Err(_) => return core::ptr::null_mut(),
    };
    unsafe { alloc(layout) }
}

#[no_mangle]
pub extern "C" fn qunix_kfree(ptr: *mut u8, size: usize) {
    use alloc::alloc::{dealloc, Layout};
    if ptr.is_null() || size == 0 { return; }
    let layout = Layout::from_size_align(size, 8).unwrap();
    unsafe { dealloc(ptr, layout); }
}

/// Physical page allocator for DMA use.
#[no_mangle]
pub extern "C" fn qunix_alloc_dma_page() -> u64 {
    crate::memory::phys::alloc_frame().unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn qunix_free_dma_page(phys: u64) {
    if phys != 0 { crate::memory::phys::free_frame(phys); }
}

/// Map physical MMIO into kernel virtual space.
#[no_mangle]
pub extern "C" fn qunix_ioremap(phys: u64, _size: usize) -> u64 {
    phys + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET
}

#[no_mangle]
pub extern "C" fn qunix_iounmap(_virt: u64) {
    // No-op for identity mapped MMIO
}

/// PCI config space read/write.
#[no_mangle]
pub extern "C" fn qunix_pci_read_config_dword(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    crate::drivers::pcie::pci_read_u32(bus, dev, func, off)
}

#[no_mangle]
pub extern "C" fn qunix_pci_write_config_dword(bus: u8, dev: u8, func: u8, off: u8, val: u32) {
    crate::drivers::pcie::pci_write_u32(bus, dev, func, off, val);
}

/// Memory barrier helpers.
#[no_mangle]
pub extern "C" fn qunix_mb()  { core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst); }
#[no_mangle]
pub extern "C" fn qunix_rmb() { core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire); }
#[no_mangle]
pub extern "C" fn qunix_wmb() { core::sync::atomic::fence(core::sync::atomic::Ordering::Release); }

/// Kernel log from driver.
#[no_mangle]
pub extern "C" fn qunix_printk(level: u8, msg: *const u8) {
    if msg.is_null() { return; }
    let s = unsafe {
        let mut len = 0; while *msg.add(len) != 0 { len += 1; }
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(msg, len))
    };
    crate::klog!("[driver] {}", s);
}

/// Udelay for driver init.
#[no_mangle]
pub extern "C" fn qunix_udelay(us: u64) {
    let ticks = us / 1000 + 1;
    let start = crate::time::ticks();
    while crate::time::ticks() - start < ticks { core::hint::spin_loop(); }
}

/// Register a block driver from a loaded module.
/// Returns assigned device ID or u32::MAX on failure.
#[no_mangle]
pub extern "C" fn qunix_register_block_driver(
    name: *const u8,
    read_fn:  extern "C" fn(u64, *mut u8, u32) -> i32,
    write_fn: extern "C" fn(u64, *const u8, u32) -> i32,
    block_sz: u32,
    num_blks: u64,
) -> u32 {
    if name.is_null() { return u32::MAX; }
    let name_str = unsafe {
        let mut l = 0; while *name.add(l) != 0 { l += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(name, l)).to_string()
    };

    struct CDriverBlock {
        name:    String,
        read_fn: extern "C" fn(u64, *mut u8, u32) -> i32,
        write_fn:extern "C" fn(u64, *const u8, u32) -> i32,
        bsz:     u32,
        nblk:    u64,
    }
    unsafe impl Send for CDriverBlock {}
    unsafe impl Sync for CDriverBlock {}

    impl BlockDriverOps for CDriverBlock {
        fn block_size(&self) -> u32 { self.bsz }
        fn num_blocks(&self) -> u64 { self.nblk }
        fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), DriverError> {
            let r = (self.read_fn)(lba, buf.as_mut_ptr(), buf.len() as u32);
            if r == 0 { Ok(()) } else { Err(DriverError::IoError(r as u32)) }
        }
        fn write(&self, lba: u64, buf: &[u8]) -> Result<(), DriverError> {
            let r = (self.write_fn)(lba, buf.as_ptr(), buf.len() as u32);
            if r == 0 { Ok(()) } else { Err(DriverError::IoError(r as u32)) }
        }
        fn name(&self) -> &str { &self.name }
    }

    let ops: Arc<dyn BlockDriverOps> = Arc::new(CDriverBlock {
        name: name_str, read_fn, write_fn, bsz: block_sz, nblk: num_blks,
    });
    register_block(ops)
}

/// Register a network driver from a loaded module.
#[no_mangle]
pub extern "C" fn qunix_register_net_driver(
    name:     *const u8,
    mac:      *const u8,
    tx_fn:    extern "C" fn(*const u8, u32) -> i32,
    poll_fn:  extern "C" fn(extern "C" fn(*const u8, u32)),
) -> u32 {
    if name.is_null() || mac.is_null() { return u32::MAX; }

    let name_str = unsafe {
        let mut l = 0; while *name.add(l) != 0 { l += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(name, l)).to_string()
    };
    let mac_bytes: [u8; 6] = unsafe { core::ptr::read(mac as *const [u8;6]) };

    struct CDriverNet {
        name:    String,
        mac:     [u8; 6],
        tx_fn:   extern "C" fn(*const u8, u32) -> i32,
        poll_fn: extern "C" fn(extern "C" fn(*const u8, u32)),
    }
    unsafe impl Send for CDriverNet {}
    unsafe impl Sync for CDriverNet {}

    impl NetDriverOps for CDriverNet {
        fn mac_address(&self) -> [u8; 6] { self.mac }
        fn transmit(&self, frame: &[u8]) -> Result<(), DriverError> {
            let r = (self.tx_fn)(frame.as_ptr(), frame.len() as u32);
            if r >= 0 { Ok(()) } else { Err(DriverError::IoError((-r) as u32)) }
        }
        fn poll_recv(&self, deliver: &mut dyn FnMut(&[u8])) {
            // The C poll function passes frames to a static callback.
            // We can't easily thread a Rust closure through C, so we use
            // a thread-local staging area.
            extern "C" fn frame_cb(data: *const u8, len: u32) {
                let slice = unsafe { core::slice::from_raw_parts(data, len as usize) };
                PENDING_FRAMES.lock().push(slice.to_vec());
            }
            (self.poll_fn)(frame_cb);
            let mut frames = PENDING_FRAMES.lock();
            for frame in frames.drain(..) { deliver(&frame); }
        }
        fn name(&self) -> &str { &self.name }
    }

    static PENDING_FRAMES: Mutex<Vec<Vec<u8>>> = Mutex::new(Vec::new());

    let ops: Arc<dyn NetDriverOps> = Arc::new(CDriverNet {
        name: name_str, mac: mac_bytes, tx_fn, poll_fn,
    });
    register_net(ops)
}
