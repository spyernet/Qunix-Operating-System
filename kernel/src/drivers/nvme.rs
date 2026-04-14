/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

//! NVMe driver — submission/completion queue I/O.
//!
//! Supports NVMe 1.4+ over PCIe. Implements:
//!   - Controller discovery via PCIe (class 0x01, subclass 0x08)
//!   - Admin queue + I/O queues
//!   - Identify controller/namespace
//!   - Read/write with physically-contiguous bounce buffers
//!   - Doorbell registers, polling completion
//!
//! LIMITATION: Uses physically contiguous bounce buffers via alloc_frames().
//! Without IOMMU support, DMA addresses equal physical addresses (only safe
//! for identity-mapped platforms like QEMU without IOMMU). On real hardware
//! with IOMMU, this driver requires IOMMU mapping before DMA.

use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;
use crate::arch::x86_64::paging::{phys_to_virt, PAGE_SIZE, KERNEL_VIRT_OFFSET};
use crate::memory::phys::{alloc_frames, free_frame, alloc_frame};
use crate::drivers::block::BlockDevice;

// ── NVMe BAR0 register offsets ────────────────────────────────────────────

const CAP:      usize = 0x000; // Controller Capabilities (64-bit)
const VS:       usize = 0x008; // Version
const INTMS:    usize = 0x00C; // Interrupt Mask Set
const INTMC:    usize = 0x010; // Interrupt Mask Clear
const CC:       usize = 0x014; // Controller Configuration
const CSTS:     usize = 0x01C; // Controller Status
const NSSR:     usize = 0x020; // NVM Subsystem Reset
const AQA:      usize = 0x024; // Admin Queue Attributes
const ASQ:      usize = 0x028; // Admin Submission Queue Base Address (64-bit)
const ACQ:      usize = 0x030; // Admin Completion Queue Base Address (64-bit)
const CMBLOC:   usize = 0x038;
const CMBSZ:    usize = 0x03C;
const BPINFO:   usize = 0x040;
const BPRSEL:   usize = 0x044;
const BPMBL:    usize = 0x048;
const PMRCAP:   usize = 0x0E00;
const PMRCTL:   usize = 0x0E04;

// Submission/Completion doorbell base = 0x1000, stride = (CAP.DSTRD + 1) * 4
const DOORBELL_BASE: usize = 0x1000;

// CC register bits
const CC_EN:    u32 = 1 << 0;
const CC_IOSQES_SHIFT: u32 = 16;
const CC_IOCQES_SHIFT: u32 = 20;
const CC_MPS_SHIFT:    u32 = 7;
const CC_CSS_NVM:      u32 = 0 << 4;

// CSTS register bits
const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1;

// ── NVMe command structures ───────────────────────────────────────────────

/// 64-byte submission queue entry.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct SqEntry {
    pub cdw0:  u32,  // Command Dword 0: OPC[7:0], FUSE[9:8], PSDT[11:10], CID[31:16]
    pub nsid:  u32,  // Namespace ID
    pub cdw2:  u32,
    pub cdw3:  u32,
    pub mptr:  u64,  // Metadata Pointer
    pub prp1:  u64,  // PRP Entry 1 (data buffer physical address)
    pub prp2:  u64,  // PRP Entry 2 (or PRP List pointer)
    pub cdw10: u32,
    pub cdw11: u32,
    pub cdw12: u32,
    pub cdw13: u32,
    pub cdw14: u32,
    pub cdw15: u32,
}

impl SqEntry {
    fn new(opcode: u8, cid: u16, nsid: u32) -> Self {
        SqEntry {
            cdw0: opcode as u32 | ((cid as u32) << 16),
            nsid,
            ..Default::default()
        }
    }

    fn set_lba(&mut self, lba: u64, nlb: u16) {
        // NLB is 0-based (0 = 1 block)
        self.cdw10 = lba as u32;
        self.cdw11 = (lba >> 32) as u32;
        self.cdw12 = (nlb as u32).saturating_sub(1);
    }
}

/// 16-byte completion queue entry.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct CqEntry {
    pub dw0:   u32,  // Command Specific
    pub dw1:   u32,  // Reserved
    pub sq_hd: u16,  // SQ Head Pointer
    pub sq_id: u16,  // SQ Identifier
    pub cid:   u16,  // Command ID
    pub phase_status: u16, // Phase Tag[0] + Status Field[15:1]
}

impl CqEntry {
    fn phase(&self) -> bool   { self.phase_status & 1 != 0 }
    fn status(&self) -> u16   { self.phase_status >> 1 }
    fn is_success(&self) -> bool { self.status() == 0 }
}

// NVMe opcodes
const OPC_ADMIN_DELETE_SQ:   u8 = 0x00;
const OPC_ADMIN_CREATE_SQ:   u8 = 0x01;
const OPC_ADMIN_GET_LOG_PAGE: u8 = 0x02;
const OPC_ADMIN_DELETE_CQ:   u8 = 0x04;
const OPC_ADMIN_CREATE_CQ:   u8 = 0x05;
const OPC_ADMIN_IDENTIFY:    u8 = 0x06;
const OPC_ADMIN_ABORT:       u8 = 0x08;
const OPC_ADMIN_SET_FEATURES: u8 = 0x09;
const OPC_ADMIN_GET_FEATURES: u8 = 0x0A;
const OPC_IO_FLUSH:          u8 = 0x00;
const OPC_IO_WRITE:          u8 = 0x01;
const OPC_IO_READ:            u8 = 0x02;

// ── Queue pair ────────────────────────────────────────────────────────────

const QUEUE_DEPTH: usize = 64;

struct Queue {
    sq_phys:   u64,
    cq_phys:   u64,
    sq:        *mut SqEntry,
    cq:        *const CqEntry,
    sq_tail:   u16,
    cq_head:   u16,
    cq_phase:  bool,
    qid:       u16,
    db_stride: usize,
    bar_virt:  u64,
    next_cid:  u16,
}

unsafe impl Send for Queue {}
unsafe impl Sync for Queue {}

impl Queue {
    fn new(qid: u16, bar_virt: u64, db_stride: usize) -> Option<Self> {
        let sq_pages = (QUEUE_DEPTH * core::mem::size_of::<SqEntry>() + 4095) / 4096;
        let cq_pages = (QUEUE_DEPTH * core::mem::size_of::<CqEntry>() + 4095) / 4096;
        let sq_phys  = alloc_frames(sq_pages)?;
        let cq_phys  = alloc_frames(cq_pages)?;
        unsafe {
            core::ptr::write_bytes(phys_to_virt(sq_phys) as *mut u8, 0, sq_pages * 4096);
            core::ptr::write_bytes(phys_to_virt(cq_phys) as *mut u8, 0, cq_pages * 4096);
        }
        Some(Queue {
            sq_phys, cq_phys,
            sq:  phys_to_virt(sq_phys) as *mut SqEntry,
            cq:  phys_to_virt(cq_phys) as *const CqEntry,
            sq_tail: 0, cq_head: 0, cq_phase: true,
            qid, db_stride, bar_virt, next_cid: 1,
        })
    }

    fn alloc_cid(&mut self) -> u16 {
        let cid = self.next_cid;
        self.next_cid = self.next_cid.wrapping_add(1).max(1);
        cid
    }

    fn sq_doorbell_offset(&self) -> usize {
        DOORBELL_BASE + self.qid as usize * 2 * self.db_stride
    }
    fn cq_doorbell_offset(&self) -> usize {
        DOORBELL_BASE + (self.qid as usize * 2 + 1) * self.db_stride
    }

    fn sq_write_doorbell(&self) {
        unsafe {
            core::ptr::write_volatile(
                (self.bar_virt + self.sq_doorbell_offset() as u64) as *mut u32,
                self.sq_tail as u32,
            );
        }
    }

    fn cq_write_doorbell(&self) {
        unsafe {
            core::ptr::write_volatile(
                (self.bar_virt + self.cq_doorbell_offset() as u64) as *mut u32,
                self.cq_head as u32,
            );
        }
    }

    /// Submit a command. Returns CID.
    unsafe fn submit(&mut self, mut cmd: SqEntry) -> u16 {
        let cid = self.alloc_cid();
        cmd.cdw0 = (cmd.cdw0 & 0xFF_FFFF) | ((cid as u32) << 16) | (cmd.cdw0 & 0xFF);
        let slot = self.sq_tail as usize;
        *self.sq.add(slot) = cmd;
        self.sq_tail = (self.sq_tail + 1) % QUEUE_DEPTH as u16;
        self.sq_write_doorbell();
        cid
    }

    /// Poll completion queue until `cid` completes. Returns status.
    unsafe fn poll_completion(&mut self, cid: u16) -> Result<CqEntry, u16> {
        const MAX_POLLS: u64 = 1_000_000;
        let mut polls = 0u64;
        loop {
            let cqe = *self.cq.add(self.cq_head as usize);
            if cqe.phase() == self.cq_phase && cqe.cid == cid {
                self.cq_head = (self.cq_head + 1) % QUEUE_DEPTH as u16;
                if self.cq_head == 0 { self.cq_phase = !self.cq_phase; }
                self.cq_write_doorbell();
                if cqe.is_success() { return Ok(cqe); }
                return Err(cqe.status());
            }
            polls += 1;
            if polls > MAX_POLLS { return Err(0xFFFF); }
            core::hint::spin_loop();
        }
    }
}

// ── Identify structures ───────────────────────────────────────────────────

#[repr(C)]
struct IdentifyController {
    vid: u16, ssvid: u16,
    sn: [u8; 20], mn: [u8; 40], fr: [u8; 8],
    rab: u8, ieee: [u8; 3], cmic: u8, mdts: u8,
    cntlid: u16, ver: u32, rtd3r: u32, rtd3e: u32,
    oaes: u32, ctratt: u32, rrls: u16,
    _reserved1: [u8; 9], cntrltype: u8,
    fguid: [u8; 16], crdt1: u16, crdt2: u16, crdt3: u16,
    _reserved2: [u8; 119], nvmsr: u8,
    // Continuing... just the fields we care about
    oacs: u16, acl: u8, aerl: u8, frmw: u8, lpa: u8,
    elpe: u8, npss: u8, avscc: u8, apsta: u8,
    wctemp: u16, cctemp: u16, mtfa: u16, hmpre: u32, hmmin: u32,
    tnvmcap: [u8; 16], unvmcap: [u8; 16],
    rpmbs: u32, edstt: u16, dsto: u8, fwug: u8,
    kas: u16, hctma: u16, mntmt: u16, mxtmt: u16,
    sanicap: u32, hmminds: u32, hmmaxd: u16,
    _reserved3: [u8; 506], // pad to 512
    sqes: u8, cqes: u8, maxcmd: u16, nn: u32,
    oncs: u16, fuses: u16, fna: u8, vwc: u8,
    awun: u16, awupf: u16, icsvscc: u8, nwpc: u8,
    acwu: u16, _reserved4: u16, sgls: u32,
    mnan: u32, _reserved5: [u8; 224],
    subnqn: [u8; 256], _reserved6: [u8; 768],
    psd: [[u8; 32]; 32],
    vs: [u8; 1024],
}

#[repr(C)]
#[derive(Default)]
struct IdentifyNamespace {
    nsze: u64,    // Namespace Size (in LBAs)
    ncap: u64,    // Namespace Capacity
    nuse: u64,    // Namespace Utilization
    nsfeat: u8, nlbaf: u8, flbas: u8, mc: u8, dpc: u8, dps: u8,
    nmic: u8, rescap: u8, fpi: u8, dlfeat: u8,
    nawun: u16, nawupf: u16, nacwu: u16, nabsn: u16,
    nabo: u16, nabspf: u16, noiob: u16,
    nvmcap: [u8; 16], npwg: u16, npwa: u16, npdg: u16, npda: u16,
    nows: u16, _reserved: [u8; 18],
    anagrpid: u32, _reserved2: [u8; 3], nsattr: u8,
    nvmsetid: u16, endgid: u16, nguid: [u8; 16], eui64: u64,
    lbaf: [[u8; 4]; 16],    // LBA Format Support (up to 64 formats)
    _vs: [u8; 16],
}

impl IdentifyNamespace {
    fn lba_size(&self) -> u32 {
        let active = self.flbas & 0xF;
        let lbaf = &self.lbaf[active as usize];
        1u32 << lbaf[1]
    }
}

// ── NVMe controller ───────────────────────────────────────────────────────

pub struct NvmeController {
    bar_virt:   u64,    // BAR0 virtual address
    db_stride:  usize,
    admin_q:    Queue,
    io_qs:      Vec<Queue>,
    nsid:       u32,
    lba_size:   u32,
    total_lbas: u64,
    model:      alloc::string::String,
}

unsafe impl Send for NvmeController {}
unsafe impl Sync for NvmeController {}

impl NvmeController {
    pub fn init(bar_phys: u64) -> Option<Self> {
        let bar_virt = bar_phys + KERNEL_VIRT_OFFSET;

        let cap = unsafe { core::ptr::read_volatile(bar_virt as *const u64) };
        let mqes = (cap & 0xFFFF) as usize + 1; // max queue entries
        let dstrd = ((cap >> 32) & 0xF) as usize;
        let db_stride = (dstrd + 1) * 4;
        let mpsmin = ((cap >> 48) & 0xF) as u32;
        let to_ms  = ((cap >> 24) & 0xFF) as u64 * 500; // timeout in ms

        // Reset controller
        let mut cc = unsafe { core::ptr::read_volatile((bar_virt + CC as u64) as *const u32) };
        cc &= !CC_EN;
        unsafe { core::ptr::write_volatile((bar_virt + CC as u64) as *mut u32, cc); }

        // Wait for not ready
        let deadline = crate::time::ticks() + to_ms + 500;
        loop {
            let csts = unsafe { core::ptr::read_volatile((bar_virt + CSTS as u64) as *const u32) };
            if csts & CSTS_RDY == 0 { break; }
            if crate::time::ticks() > deadline { return None; }
            core::hint::spin_loop();
        }

        // Admin queue depth = min(QUEUE_DEPTH, mqes)
        let adepth = QUEUE_DEPTH.min(mqes);

        // Allocate admin queue
        let admin_q = Queue::new(0, bar_virt, db_stride)?;

        // Set admin queue attributes
        let aqa = ((adepth - 1) as u32) << 16 | ((adepth - 1) as u32);
        unsafe {
            core::ptr::write_volatile((bar_virt + AQA as u64) as *mut u32, aqa);
            core::ptr::write_volatile((bar_virt + ASQ as u64) as *mut u64, admin_q.sq_phys);
            core::ptr::write_volatile((bar_virt + ACQ as u64) as *mut u64, admin_q.cq_phys);
        }

        // Configure and enable controller
        // SQES=6 (64B), CQES=4 (16B), MPS=0 (4KB)
        cc = CC_EN | (6 << CC_IOSQES_SHIFT) | (4 << CC_IOCQES_SHIFT) | CC_CSS_NVM;
        unsafe { core::ptr::write_volatile((bar_virt + CC as u64) as *mut u32, cc); }

        // Wait for ready
        let deadline = crate::time::ticks() + to_ms + 500;
        loop {
            let csts = unsafe { core::ptr::read_volatile((bar_virt + CSTS as u64) as *const u32) };
            if csts & CSTS_RDY != 0 { break; }
            if csts & CSTS_CFS != 0 { return None; } // fatal error
            if crate::time::ticks() > deadline { return None; }
            core::hint::spin_loop();
        }

        let mut ctrl = NvmeController {
            bar_virt, db_stride, admin_q,
            io_qs: Vec::new(), nsid: 1, lba_size: 512, total_lbas: 0,
            model: alloc::string::String::from("NVMe"),
        };

        // Identify controller
        ctrl.identify_controller();

        // Create 1 I/O queue
        ctrl.create_io_queues(1);

        // Identify namespace 1
        ctrl.identify_namespace(1);

        crate::klog!("NVMe: {} {}MB ({} * {}B blocks)",
            ctrl.model,
            ctrl.total_lbas * ctrl.lba_size as u64 / 1024 / 1024,
            ctrl.total_lbas, ctrl.lba_size);

        Some(ctrl)
    }

    fn identify_controller(&mut self) {
        let buf_phys = match alloc_frame() { Some(p) => p, None => return };
        unsafe { core::ptr::write_bytes(phys_to_virt(buf_phys) as *mut u8, 0, 4096); }

        let mut cmd = SqEntry::new(OPC_ADMIN_IDENTIFY, 0, 0);
        cmd.prp1 = buf_phys;
        cmd.cdw10 = 1; // CNS = 1 (identify controller)

        let cid = unsafe { self.admin_q.submit(cmd) };
        if let Ok(_) = unsafe { self.admin_q.poll_completion(cid) } {
            let ident = unsafe { &*(phys_to_virt(buf_phys) as *const IdentifyController) };
            let mn = &ident.mn;
            let end = mn.iter().rposition(|&b| b != b' ').map(|i| i + 1).unwrap_or(0);
            self.model = alloc::string::String::from_utf8_lossy(&mn[..end]).into_owned();
        }
        free_frame(buf_phys);
    }

    fn identify_namespace(&mut self, nsid: u32) {
        let buf_phys = match alloc_frame() { Some(p) => p, None => return };
        unsafe { core::ptr::write_bytes(phys_to_virt(buf_phys) as *mut u8, 0, 4096); }

        let mut cmd = SqEntry::new(OPC_ADMIN_IDENTIFY, 0, nsid);
        cmd.prp1 = buf_phys;
        cmd.cdw10 = 0; // CNS = 0 (identify namespace)

        let cid = unsafe { self.admin_q.submit(cmd) };
        if let Ok(_) = unsafe { self.admin_q.poll_completion(cid) } {
            let ns = unsafe { &*(phys_to_virt(buf_phys) as *const IdentifyNamespace) };
            self.total_lbas = ns.nsze;
            self.lba_size   = ns.lba_size();
            self.nsid       = nsid;
        }
        free_frame(buf_phys);
    }

    fn create_io_queues(&mut self, count: u16) {
        for i in 1..=count {
            let mut ioq = match Queue::new(i, self.bar_virt, self.db_stride) {
                Some(q) => q,
                None    => continue,
            };

            // Create completion queue
            let mut cmd = SqEntry::new(OPC_ADMIN_CREATE_CQ, 0, 0);
            cmd.prp1 = ioq.cq_phys;
            cmd.cdw10 = ((QUEUE_DEPTH as u32 - 1) << 16) | i as u32;
            cmd.cdw11 = 0x3; // physically contiguous + IEN
            let cid = unsafe { self.admin_q.submit(cmd) };
            if unsafe { self.admin_q.poll_completion(cid) }.is_err() { continue; }

            // Create submission queue
            let mut cmd = SqEntry::new(OPC_ADMIN_CREATE_SQ, 0, 0);
            cmd.prp1 = ioq.sq_phys;
            cmd.cdw10 = ((QUEUE_DEPTH as u32 - 1) << 16) | i as u32;
            cmd.cdw11 = 0x1 | ((i as u32) << 16); // physically contiguous + CQID
            let cid = unsafe { self.admin_q.submit(cmd) };
            if unsafe { self.admin_q.poll_completion(cid) }.is_err() { continue; }

            self.io_qs.push(ioq);
        }
    }

    fn io_queue(&mut self) -> Option<&mut Queue> {
        self.io_qs.get_mut(0)
    }

    /// Synchronous read `count` LBAs from `lba` into `buf`.
    pub fn read_lbas(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        let lba_sz = self.lba_size as usize;
        if buf.len() % lba_sz != 0 { return Err("unaligned buffer"); }
        let nlb = buf.len() / lba_sz;

        // Allocate bounce buffer (physically contiguous)
        let pages  = (buf.len() + 4095) / 4096;
        let b_phys = alloc_frames(pages).ok_or("no memory")?;
        unsafe { core::ptr::write_bytes(phys_to_virt(b_phys) as *mut u8, 0, pages * 4096); }

        let mut cmd = SqEntry::new(OPC_IO_READ, 0, self.nsid);
        cmd.prp1 = b_phys;
        // PRP2 required for >4KB transfers: alloc_frames() gives us contiguous
        // physical pages, so PRP2 = PRP1 + PAGE_SIZE is correct for 2-page transfers.
        // For >2 pages we would need a PRP list; we cap at 2 pages (8KB) here.
        if pages > 1 { cmd.prp2 = b_phys + PAGE_SIZE; }
        if pages > 2 {
            // Transfers > 8KB require PRP list; reject to avoid data corruption
            for i in 0..pages { free_frame(b_phys + i as u64 * PAGE_SIZE); }
            return Err("NVMe read >8KB not yet supported");
        }
        cmd.set_lba(lba, nlb as u16);

        let ioq = self.io_queue().ok_or("no IO queue")?;
        let cid = unsafe { ioq.submit(cmd) };
        unsafe { ioq.poll_completion(cid) }.map_err(|_| "NVMe read error")?;

        unsafe {
            core::ptr::copy_nonoverlapping(
                phys_to_virt(b_phys) as *const u8,
                buf.as_mut_ptr(),
                buf.len(),
            );
        }
        for i in 0..pages { free_frame(b_phys + i as u64 * PAGE_SIZE); }
        Ok(())
    }

    /// Synchronous write `buf` to LBA `lba`.
    pub fn write_lbas(&mut self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        let lba_sz = self.lba_size as usize;
        if buf.len() % lba_sz != 0 { return Err("unaligned buffer"); }
        let nlb = buf.len() / lba_sz;

        let pages  = (buf.len() + 4095) / 4096;
        let b_phys = alloc_frames(pages).ok_or("no memory")?;
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                phys_to_virt(b_phys) as *mut u8,
                buf.len(),
            );
        }

        let mut cmd = SqEntry::new(OPC_IO_WRITE, 0, self.nsid);
        cmd.prp1 = b_phys;
        if pages > 1 { cmd.prp2 = b_phys + PAGE_SIZE; }
        cmd.set_lba(lba, nlb as u16);

        let ioq = self.io_queue().ok_or("no IO queue")?;
        let cid = unsafe { ioq.submit(cmd) };
        unsafe { ioq.poll_completion(cid) }.map_err(|_| "NVMe write error")?;

        for i in 0..pages { free_frame(b_phys + i as u64 * PAGE_SIZE); }
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), &'static str> {
        let mut cmd = SqEntry::new(OPC_IO_FLUSH, 0, self.nsid);
        let ioq = self.io_queue().ok_or("no IO queue")?;
        let cid = unsafe { ioq.submit(cmd) };
        unsafe { ioq.poll_completion(cid) }.map_err(|_| "flush error")?;
        Ok(())
    }
}

// ── BlockDevice implementation ────────────────────────────────────────────

pub struct NvmeDisk {
    ctrl:       Mutex<NvmeController>,
    block_size: usize,
    blocks:     u64,
}

impl NvmeDisk {
    pub fn new(ctrl: NvmeController) -> Arc<Self> {
        let bs = ctrl.lba_size as usize;
        let n  = ctrl.total_lbas;
        Arc::new(NvmeDisk { ctrl: Mutex::new(ctrl), block_size: bs, blocks: n })
    }
}

impl BlockDevice for NvmeDisk {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        self.ctrl.lock().read_lbas(lba, buf)
    }
    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        self.ctrl.lock().write_lbas(lba, buf)
    }
    fn block_size(&self) -> usize { self.block_size }
    fn total_blocks(&self) -> u64 { self.blocks }
}

// ── Driver registration ───────────────────────────────────────────────────

static NVME_DEVICES: Mutex<Vec<Arc<NvmeDisk>>> = Mutex::new(Vec::new());

/// Called by PCI enumeration when an NVMe device is found.
pub fn register_device(bar0_phys: u64) {
    match NvmeController::init(bar0_phys) {
        Some(ctrl) => {
            let disk = NvmeDisk::new(ctrl);
            // Register with driver isolation host for clean kernel/driver boundary
            crate::drivers::driver_host::register_block(disk.clone());
            crate::drivers::block::register_nvme(disk.clone());
            NVME_DEVICES.lock().push(disk);
        }
        None => crate::klog!("NVMe: init failed for BAR0={:#x}", bar0_phys),
    }
}

pub fn device_count() -> usize { NVME_DEVICES.lock().len() }

pub fn get_device(idx: usize) -> Option<Arc<NvmeDisk>> {
    NVME_DEVICES.lock().get(idx).cloned()
}

impl crate::drivers::driver_host::BlockDriverOps for NvmeDisk {
    fn block_size(&self) -> u32 { 512 }
    fn num_blocks(&self) -> u64 { 0 }
    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), crate::drivers::driver_host::DriverError> {
        let _ = (lba, buf);
        Err(crate::drivers::driver_host::DriverError::IoError(0))
    }
    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), crate::drivers::driver_host::DriverError> {
        let _ = (lba, buf);
        Err(crate::drivers::driver_host::DriverError::IoError(0))
    }
    fn name(&self) -> &str { "nvme" }
}
