use alloc::vec::Vec;
use spin::Mutex;
use crate::memory::phys::alloc_frame;
use crate::arch::x86_64::paging::{phys_to_virt, PAGE_SIZE};

const SLAB_MAGIC: u32 = 0x514E5358; // QNSX

struct SlabPage {
    base:    u64,
    free:    Vec<u64>,
    used:    usize,
    cap:     usize,
}

impl SlabPage {
    fn new(obj_size: usize) -> Option<Self> {
        let phys = alloc_frame()?;
        let base = phys_to_virt(phys);
        unsafe { core::ptr::write_bytes(base as *mut u8, 0, PAGE_SIZE as usize); }
        let cap = PAGE_SIZE as usize / obj_size;
        let mut free = Vec::with_capacity(cap);
        for i in 0..cap {
            free.push(base + (i * obj_size) as u64);
        }
        Some(SlabPage { base, free, used: 0, cap })
    }
}

pub struct SlabCache {
    obj_size: usize,
    pages:    Mutex<Vec<SlabPage>>,
}

impl SlabCache {
    pub const fn new(obj_size: usize) -> Self {
        SlabCache { obj_size, pages: Mutex::new(Vec::new()) }
    }

    pub fn alloc(&self) -> Option<*mut u8> {
        let mut pages = self.pages.lock();
        // Find page with free slot
        for page in pages.iter_mut() {
            if let Some(ptr) = page.free.pop() {
                page.used += 1;
                return Some(ptr as *mut u8);
            }
        }
        // Grow: add a new slab page
        let new_page = SlabPage::new(self.obj_size)?;
        pages.push(new_page);
        let page = pages.last_mut()?;
        let ptr = page.free.pop()?;
        page.used += 1;
        Some(ptr as *mut u8)
    }

    pub fn free(&self, ptr: *mut u8) {
        let addr = ptr as u64;
        let mut pages = self.pages.lock();
        for page in pages.iter_mut() {
            let end = page.base + PAGE_SIZE;
            if addr >= page.base && addr < end {
                page.free.push(addr);
                if page.used > 0 { page.used -= 1; }
                return;
            }
        }
    }

    pub fn used_objects(&self) -> usize {
        self.pages.lock().iter().map(|p| p.used).sum()
    }
}

// Global slab caches for common kernel object sizes
pub static SLAB_16:   SlabCache = SlabCache::new(16);
pub static SLAB_32:   SlabCache = SlabCache::new(32);
pub static SLAB_64:   SlabCache = SlabCache::new(64);
pub static SLAB_128:  SlabCache = SlabCache::new(128);
pub static SLAB_256:  SlabCache = SlabCache::new(256);
pub static SLAB_512:  SlabCache = SlabCache::new(512);
pub static SLAB_1024: SlabCache = SlabCache::new(1024);
pub static SLAB_2048: SlabCache = SlabCache::new(2048);

pub fn alloc_sized(size: usize) -> Option<*mut u8> {
    match size {
        0..=16   => SLAB_16.alloc(),
        17..=32  => SLAB_32.alloc(),
        33..=64  => SLAB_64.alloc(),
        65..=128 => SLAB_128.alloc(),
        129..=256 => SLAB_256.alloc(),
        257..=512 => SLAB_512.alloc(),
        513..=1024 => SLAB_1024.alloc(),
        1025..=2048 => SLAB_2048.alloc(),
        _ => None,
    }
}

pub fn free_sized(ptr: *mut u8, size: usize) {
    match size {
        0..=16   => SLAB_16.free(ptr),
        17..=32  => SLAB_32.free(ptr),
        33..=64  => SLAB_64.free(ptr),
        65..=128 => SLAB_128.free(ptr),
        129..=256 => SLAB_256.free(ptr),
        257..=512 => SLAB_512.free(ptr),
        513..=1024 => SLAB_1024.free(ptr),
        1025..=2048 => SLAB_2048.free(ptr),
        _ => {}
    }
}
