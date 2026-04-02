use linked_list_allocator::LockedHeap;
use crate::arch::x86_64::paging::{
    PageFlags, PageMapper, KERNEL_HEAP_START, KERNEL_HEAP_SIZE, PAGE_SIZE,
};
use crate::memory::phys::alloc_frame;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

pub fn init() {
    let heap_start = KERNEL_HEAP_START;
    let free_bytes = crate::memory::phys::free_frames() as u64 * PAGE_SIZE;
    let heap_size = free_bytes
        .saturating_div(8)
        .clamp(16 * 1024 * 1024, 64 * 1024 * 1024)
        .min(KERNEL_HEAP_SIZE);
    let pages = heap_size / PAGE_SIZE;

    let flags = PageFlags::PRESENT | PageFlags::WRITABLE | PageFlags::NO_EXECUTE;
    let mut mapper = PageMapper::current();

    for i in 0..pages {
        let virt = heap_start + i * PAGE_SIZE;
        let phys = alloc_frame().expect("OOM during heap init");
        unsafe { mapper.map_page(virt, phys, flags); }
    }

    unsafe {
        ALLOCATOR.lock().init(heap_start as *mut u8, heap_size as usize);
    }

    crate::klog!("Kernel heap mapped: {} MiB", heap_size / 1024 / 1024);
}

pub fn used() -> usize {
    ALLOCATOR.lock().used()
}

pub fn free() -> usize {
    ALLOCATOR.lock().free()
}
