use spin::Mutex;
use crate::arch::x86_64::port::{inb, outb, inw, outw};

pub trait BlockDevice: Send + Sync {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str>;
    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str>;
    fn block_size(&self) -> usize;
    fn total_blocks(&self) -> u64;
}

const ATA_PRIMARY_BASE: u16 = 0x1F0;
const ATA_STATUS:       u16 = ATA_PRIMARY_BASE + 7;
const ATA_COMMAND:      u16 = ATA_PRIMARY_BASE + 7;
const ATA_DATA:         u16 = ATA_PRIMARY_BASE;
const ATA_ERROR:        u16 = ATA_PRIMARY_BASE + 1;
const ATA_SECTOR_COUNT: u16 = ATA_PRIMARY_BASE + 2;
const ATA_LBA_LO:       u16 = ATA_PRIMARY_BASE + 3;
const ATA_LBA_MID:      u16 = ATA_PRIMARY_BASE + 4;
const ATA_LBA_HI:       u16 = ATA_PRIMARY_BASE + 5;
const ATA_DRIVE_HEAD:   u16 = ATA_PRIMARY_BASE + 6;

const ATA_CMD_READ_PIO:  u8 = 0x20;
const ATA_CMD_WRITE_PIO: u8 = 0x30;
const ATA_STATUS_BSY:    u8 = 0x80;
const ATA_STATUS_DRQ:    u8 = 0x08;
const ATA_STATUS_ERR:    u8 = 0x01;

pub struct AtaDrive {
    base: u16,
    slave: bool,
}

impl AtaDrive {
    pub fn new(base: u16, slave: bool) -> Self {
        AtaDrive { base, slave }
    }

    fn wait_not_busy(&self) {
        unsafe {
            while inb(self.base + 7) & ATA_STATUS_BSY != 0 {}
        }
    }

    fn wait_drq(&self) -> Result<(), &'static str> {
        unsafe {
            loop {
                let status = inb(self.base + 7);
                if status & ATA_STATUS_ERR != 0 { return Err("ATA error"); }
                if status & ATA_STATUS_DRQ != 0 { return Ok(()); }
            }
        }
    }
}

impl BlockDevice for AtaDrive {
    fn read_block(&self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
        if buf.len() < 512 { return Err("Buffer too small"); }
        self.wait_not_busy();
        unsafe {
            let drive = if self.slave { 0xF0 } else { 0xE0 };
            outb(self.base + 6, drive | ((lba >> 24) as u8 & 0x0F));
            outb(self.base + 2, 1);
            outb(self.base + 3, lba as u8);
            outb(self.base + 4, (lba >> 8) as u8);
            outb(self.base + 5, (lba >> 16) as u8);
            outb(self.base + 7, ATA_CMD_READ_PIO);
        }
        self.wait_drq()?;
        unsafe {
            let ptr = buf.as_mut_ptr() as *mut u16;
            for i in 0..256 {
                *ptr.add(i) = inw(self.base);
            }
        }
        Ok(())
    }

    fn write_block(&self, lba: u64, buf: &[u8]) -> Result<(), &'static str> {
        if buf.len() < 512 { return Err("Buffer too small"); }
        self.wait_not_busy();
        unsafe {
            let drive = if self.slave { 0xF0 } else { 0xE0 };
            outb(self.base + 6, drive | ((lba >> 24) as u8 & 0x0F));
            outb(self.base + 2, 1);
            outb(self.base + 3, lba as u8);
            outb(self.base + 4, (lba >> 8) as u8);
            outb(self.base + 5, (lba >> 16) as u8);
            outb(self.base + 7, ATA_CMD_WRITE_PIO);
        }
        self.wait_drq()?;
        unsafe {
            let ptr = buf.as_ptr() as *const u16;
            for i in 0..256 {
                outw(self.base, *ptr.add(i));
            }
            outb(self.base + 7, 0xE7); // cache flush
        }
        Ok(())
    }

    fn block_size(&self) -> usize { 512 }
    fn total_blocks(&self) -> u64 { 0 } // TODO: IDENTIFY command
}

use alloc::sync::Arc;
// duplicate Mutex removed

static NVME_DRIVES: Mutex<alloc::vec::Vec<Arc<crate::drivers::nvme::NvmeDisk>>> = Mutex::new(alloc::vec::Vec::new());

pub fn register_nvme(disk: Arc<crate::drivers::nvme::NvmeDisk>) {
    NVME_DRIVES.lock().push(disk);
    crate::klog!("block: NVMe drive registered ({} total)", NVME_DRIVES.lock().len());
}

pub fn get_nvme(idx: usize) -> Option<Arc<crate::drivers::nvme::NvmeDisk>> {
    NVME_DRIVES.lock().get(idx).cloned()
}

pub fn nvme_count() -> usize { NVME_DRIVES.lock().len() }
