//! virtio-net driver — real descriptor ring implementation.
//!
//! Supports:
//! - PCI virtio-net device detection (vendor 0x1AF4, device 0x1000)
//! - Legacy virtio device initialization
//! - TX/RX virtqueues with 16-descriptor rings
//! - Loopback path when no NIC found

use alloc::vec::Vec;
use alloc::collections::VecDeque;
use spin::Mutex;
use crate::arch::x86_64::port::{inb, outb, inw, outw, inl, outl};
use crate::arch::x86_64::paging::{phys_to_virt, PAGE_SIZE};
use crate::memory::phys::alloc_frame;

// ── virtio PCI legacy register offsets ───────────────────────────────────

const VIRTIO_PCI_HOST_FEATURES:  u16 = 0;
const VIRTIO_PCI_GUEST_FEATURES: u16 = 4;
const VIRTIO_PCI_QUEUE_PFN:      u16 = 8;
const VIRTIO_PCI_QUEUE_NUM:      u16 = 12;
const VIRTIO_PCI_QUEUE_SEL:      u16 = 14;
const VIRTIO_PCI_QUEUE_NOTIFY:   u16 = 16;
const VIRTIO_PCI_STATUS:         u16 = 18;
const VIRTIO_PCI_ISR:            u16 = 19;
const VIRTIO_PCI_CONFIG:         u16 = 20;   // net-specific config starts here

const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1;
const VIRTIO_STATUS_DRIVER:      u8 = 2;
const VIRTIO_STATUS_DRIVER_OK:   u8 = 4;
const VIRTIO_STATUS_FAILED:      u8 = 128;

const VIRTIO_NET_F_MAC:          u32 = 1 << 5;
const VIRTIO_NET_F_STATUS:       u32 = 1 << 16;

const QUEUE_SIZE: usize = 16;

// ── Virtqueue structures ──────────────────────────────────────────────────

#[repr(C)]
struct VirtqDesc {
    addr:  u64,   // guest physical address of buffer
    len:   u32,   // length
    flags: u16,   // NEXT=1, WRITE=2, INDIRECT=4
    next:  u16,   // index of next descriptor if NEXT set
}

const VRING_DESC_F_NEXT:  u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;  // device-writable (RX buffers)

#[repr(C)]
struct VirtqAvail {
    flags:  u16,
    idx:    u16,
    ring:   [u16; QUEUE_SIZE],
    used_event: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtqUsedElem {
    id:  u32,
    len: u32,
}

#[repr(C)]
struct VirtqUsed {
    flags:      u16,
    idx:        u16,
    ring:       [VirtqUsedElem; QUEUE_SIZE],
    avail_event: u16,
}

struct Virtqueue {
    // Physical base of the virtqueue page
    phys:   u64,
    // Virtual pointers into the page
    desc:   *mut VirtqDesc,
    avail:  *mut VirtqAvail,
    used:   *mut VirtqUsed,
    // Software state
    free_head:    usize,
    free_count:   usize,
    last_used_idx: u16,
    // For each descriptor: associated buffer (for freeing on completion)
    buf_addrs: [u64; QUEUE_SIZE],
    buf_lens:  [usize; QUEUE_SIZE],
}

unsafe impl Send for Virtqueue {}
unsafe impl Sync for Virtqueue {}

// Layout: descriptors first, then avail, then used (4096-aligned)
const DESC_OFFSET:  usize = 0;
const AVAIL_OFFSET: usize = QUEUE_SIZE * 16;  // 16 bytes per desc
const USED_OFFSET:  usize = 4096;             // used ring on second page

impl Virtqueue {
    unsafe fn new() -> Option<Self> {
        // Allocate 2 pages: one for desc+avail, one for used
        let phys = alloc_frame()?;
        let phys2 = alloc_frame()?;
        let virt = phys_to_virt(phys);
        let virt2 = phys_to_virt(phys2);
        core::ptr::write_bytes(virt as *mut u8, 0, PAGE_SIZE as usize);
        core::ptr::write_bytes(virt2 as *mut u8, 0, PAGE_SIZE as usize);

        let desc  = (virt + DESC_OFFSET as u64)  as *mut VirtqDesc;
        let avail = (virt + AVAIL_OFFSET as u64) as *mut VirtqAvail;
        let used  = virt2 as *mut VirtqUsed;

        // Chain all descriptors into free list
        for i in 0..QUEUE_SIZE {
            (*desc.add(i)).next = (i + 1) as u16;
        }

        Some(Virtqueue {
            phys, desc, avail, used,
            free_head: 0, free_count: QUEUE_SIZE, last_used_idx: 0,
            buf_addrs: [0; QUEUE_SIZE], buf_lens: [0; QUEUE_SIZE],
        })
    }

    /// Place `queue_index` in the device's Queue PFN register.
    /// Legacy virtio uses 4096-byte pages, PFN = phys >> 12.
    fn pfn(&self) -> u32 { (self.phys >> 12) as u32 }

    unsafe fn alloc_desc(&mut self) -> Option<usize> {
        if self.free_count == 0 { return None; }
        let idx = self.free_head;
        self.free_head = (*self.desc.add(idx)).next as usize;
        self.free_count -= 1;
        Some(idx)
    }

    unsafe fn free_desc(&mut self, idx: usize) {
        (*self.desc.add(idx)).next = self.free_head as u16;
        self.free_head = idx;
        self.free_count += 1;
    }

    /// Enqueue a TX buffer into the available ring.
    unsafe fn enqueue_tx(&mut self, data_phys: u64, len: usize) -> bool {
        // virtio-net TX: virtio header (10 bytes) + data
        let hdr_idx  = match self.alloc_desc() { Some(i) => i, None => return false };
        let data_idx = match self.alloc_desc() { Some(i) => i, None => {
            self.free_desc(hdr_idx); return false;
        }};

        // Virtio-net header (10 bytes, all zeros for basic TX)
        let hdr_frame = match alloc_frame() { Some(f) => f, None => {
            self.free_desc(hdr_idx); self.free_desc(data_idx); return false;
        }};
        core::ptr::write_bytes(phys_to_virt(hdr_frame) as *mut u8, 0, 12);

        // Header descriptor
        (*self.desc.add(hdr_idx)) = VirtqDesc {
            addr: hdr_frame, len: 12,
            flags: VRING_DESC_F_NEXT, next: data_idx as u16,
        };
        // Data descriptor
        (*self.desc.add(data_idx)) = VirtqDesc {
            addr: data_phys, len: len as u32,
            flags: 0, next: 0,
        };
        self.buf_addrs[hdr_idx] = hdr_frame;
        self.buf_lens[hdr_idx]  = 12;

        // Publish to avail ring
        let avail_idx = (*self.avail).idx as usize % QUEUE_SIZE;
        (*self.avail).ring[avail_idx] = hdr_idx as u16;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        (*self.avail).idx = (*self.avail).idx.wrapping_add(1);
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        true
    }

    /// Enqueue an RX buffer (device-writable).
    unsafe fn enqueue_rx(&mut self, buf_phys: u64, len: usize) -> bool {
        let idx = match self.alloc_desc() { Some(i) => i, None => return false };
        (*self.desc.add(idx)) = VirtqDesc {
            addr: buf_phys, len: len as u32,
            flags: VRING_DESC_F_WRITE, next: 0,
        };
        self.buf_addrs[idx] = buf_phys;
        self.buf_lens[idx]  = len;

        let avail_idx = (*self.avail).idx as usize % QUEUE_SIZE;
        (*self.avail).ring[avail_idx] = idx as u16;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
        (*self.avail).idx = (*self.avail).idx.wrapping_add(1);
        true
    }

    /// Collect completed descriptors from the used ring.
    unsafe fn collect_used(&mut self) -> Vec<(usize, u32)> {
        let mut completed = Vec::new();
        loop {
            let used_idx = (*self.used).idx;
            if self.last_used_idx == used_idx { break; }
            let slot = self.last_used_idx as usize % QUEUE_SIZE;
            let elem = (*self.used).ring[slot].clone();
            self.last_used_idx = self.last_used_idx.wrapping_add(1);
            completed.push((elem.id as usize, elem.len));
        }
        completed
    }
}

// ── NIC state ─────────────────────────────────────────────────────────────

struct VirtioNic {
    io_base: u16,
    mac:     [u8; 6],
    txq:     Virtqueue,
    rxq:     Virtqueue,
    // Receive buffers: vec of (phys, virt, len)
    rx_bufs: Vec<(u64, u64, usize)>,
    active:  bool,
}

unsafe impl Send for VirtioNic {}

const RX_BUF_SIZE: usize = 1526; // MTU 1500 + virtio header
const RX_BUFS:     usize = 8;

impl VirtioNic {
    unsafe fn init(io_base: u16) -> Option<Self> {
        // Reset device
        outb(io_base + VIRTIO_PCI_STATUS, 0);
        outb(io_base + VIRTIO_PCI_STATUS, VIRTIO_STATUS_ACKNOWLEDGE);
        outb(io_base + VIRTIO_PCI_STATUS, VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER);

        // Read host features, accept what we need
        let host_features = inl(io_base + VIRTIO_PCI_HOST_FEATURES);
        let guest_features = host_features & (VIRTIO_NET_F_MAC | VIRTIO_NET_F_STATUS);
        outl(io_base + VIRTIO_PCI_GUEST_FEATURES, guest_features);

        // Read MAC address
        let mut mac = [0u8; 6];
        if guest_features & VIRTIO_NET_F_MAC != 0 {
            for i in 0..6 {
                mac[i] = inb(io_base + VIRTIO_PCI_CONFIG + i as u16);
            }
        }

        // Set up RX queue (queue 0)
        outw(io_base + VIRTIO_PCI_QUEUE_SEL, 0);
        let rxq_size = inw(io_base + VIRTIO_PCI_QUEUE_NUM) as usize;
        let mut rxq = Virtqueue::new()?;
        outl(io_base + VIRTIO_PCI_QUEUE_PFN, rxq.pfn());

        // Set up TX queue (queue 1)
        outw(io_base + VIRTIO_PCI_QUEUE_SEL, 1);
        let txq_size = inw(io_base + VIRTIO_PCI_QUEUE_NUM) as usize;
        let mut txq = Virtqueue::new()?;
        outl(io_base + VIRTIO_PCI_QUEUE_PFN, txq.pfn());

        // Driver OK
        outb(io_base + VIRTIO_PCI_STATUS,
             VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK);

        // Pre-fill RX ring
        let mut rx_bufs = Vec::new();
        for _ in 0..RX_BUFS {
            if let Some(phys) = alloc_frame() {
                let virt = phys_to_virt(phys);
                core::ptr::write_bytes(virt as *mut u8, 0, PAGE_SIZE as usize);
                rxq.enqueue_rx(phys, RX_BUF_SIZE);
                rx_bufs.push((phys, virt, RX_BUF_SIZE));
            }
        }
        // Notify device of new RX buffers
        outw(io_base + VIRTIO_PCI_QUEUE_NOTIFY, 0);

        Some(VirtioNic { io_base, mac, txq, rxq, rx_bufs, active: true })
    }

    unsafe fn transmit(&mut self, frame: &[u8]) -> bool {
        // Copy frame to a physical page
        if frame.len() > 1514 { return false; }
        let phys = match alloc_frame() { Some(p) => p, None => return false };
        let virt = phys_to_virt(phys);
        core::ptr::copy_nonoverlapping(frame.as_ptr(), virt as *mut u8, frame.len());

        let ok = self.txq.enqueue_tx(phys, frame.len());
        if ok {
            core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
            outw(self.io_base + VIRTIO_PCI_QUEUE_NOTIFY, 1); // notify TX queue
        }
        // Collect TX completions (free buffers)
        for (desc_idx, _len) in self.txq.collect_used() {
            let buf_phys = self.txq.buf_addrs[desc_idx];
            if buf_phys != 0 {
                crate::memory::phys::free_frame(buf_phys);
            }
            self.txq.free_desc(desc_idx);
        }
        ok
    }

    unsafe fn poll_rx(&mut self) {
        let completed = self.rxq.collect_used();
        for (desc_idx, len) in completed.iter().copied() {
            let buf_phys = self.rxq.buf_addrs[desc_idx];
            let buf_virt = phys_to_virt(buf_phys);
            // virtio-net header is 12 bytes; skip it
            let data_start = buf_virt + 12;
            let data_len   = (len as usize).saturating_sub(12);
            if data_len > 0 {
                let packet = core::slice::from_raw_parts(data_start as *const u8, data_len);
                crate::net::ip::receive(packet);
            }
            // Re-enqueue the buffer
            self.rxq.free_desc(desc_idx);
            core::ptr::write_bytes(buf_virt as *mut u8, 0, RX_BUF_SIZE);
            self.rxq.enqueue_rx(buf_phys, RX_BUF_SIZE);
        }
        // Notify device of refilled RX buffers
        if !completed.is_empty() {
            outw(self.io_base + VIRTIO_PCI_QUEUE_NOTIFY, 0);
        }
    }
}

// ── Driver state ──────────────────────────────────────────────────────────

static NIC: Mutex<Option<VirtioNic>> = Mutex::new(None);

// Loopback queue when no NIC
static LOOPBACK: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());

pub fn init() {
    // Try to find a virtio-net device from PCI enumeration
    if let Some(dev) = crate::drivers::pcie::find_device(0x1AF4, 0x1000) {
        // BAR0 is IO space for legacy virtio
        let io_base = dev.bar_addr(0) as u16;
        if io_base != 0 {
            unsafe {
                match VirtioNic::init(io_base) {
                    Some(nic) => {
                        let mac = nic.mac;
                        *NIC.lock() = Some(nic);
                        crate::klog!("virtio-net: MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} io={:#x}",
                            mac[0],mac[1],mac[2],mac[3],mac[4],mac[5], io_base);
                        return;
                    }
                    None => crate::klog!("virtio-net: init failed at io={:#x}", io_base),
                }
            }
        }
    }
    crate::klog!("virtio-net: no device found, using loopback");
}

pub fn transmit(frame: &[u8]) {
    if let Some(nic) = NIC.lock().as_mut() {
        unsafe { nic.transmit(frame); }
        return;
    }
    // Loopback path
    let local = crate::net::ip::local_ip();
    if frame.len() >= 20 {
        let dst_ip = u32::from_be_bytes(frame[16..20].try_into().unwrap_or([0;4]));
        if dst_ip == local || dst_ip == u32::from_be_bytes([127,0,0,1]) {
            LOOPBACK.lock().push_back(frame.to_vec());
        }
    }
    drain_loopback();
}

pub fn receive_poll() {
    if let Some(nic) = NIC.lock().as_mut() {
        unsafe { nic.poll_rx(); }
    }
    drain_loopback();
}

fn drain_loopback() {
    loop {
        let pkt = LOOPBACK.lock().pop_front();
        match pkt {
            Some(p) => crate::net::ip::receive(&p),
            None    => break,
        }
    }
}

pub fn mac_address() -> [u8; 6] {
    NIC.lock().as_ref().map(|n| n.mac).unwrap_or([0x52,0x54,0x00,0x12,0x34,0x56])
}

pub fn is_up() -> bool { NIC.lock().is_some() }
