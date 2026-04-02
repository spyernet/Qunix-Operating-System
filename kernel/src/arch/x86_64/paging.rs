use bitflags::bitflags;
use crate::memory::phys::alloc_frame;

bitflags! {
    #[derive(Clone, Copy)]
    pub struct PageFlags: u64 {
        const PRESENT    = 1 << 0;
        const WRITABLE   = 1 << 1;
        const USER       = 1 << 2;
        const WRITE_THROUGH = 1 << 3;
        const NO_CACHE   = 1 << 4;
        const ACCESSED   = 1 << 5;
        const DIRTY      = 1 << 6;
        const HUGE       = 1 << 7;
        const GLOBAL     = 1 << 8;
        /// Software-defined: page is COW (Copy-on-Write).
        /// Set on read-only shared frames; cleared when a private copy is made.
        /// Bit 9 is available to software in page-table entries (ignored by CPU).
        const COW        = 1 << 9;
        const NO_EXECUTE = 1 << 63;
    }
}

pub const PAGE_SIZE: u64 = 4096;
pub const KERNEL_VIRT_OFFSET: u64 = 0xFFFF_8000_0000_0000;
pub const KERNEL_HEAP_START: u64 = 0xFFFF_C000_0000_0000;
pub const KERNEL_HEAP_SIZE: u64 = 0x4000_0000;
pub const USER_STACK_TOP: u64 = 0x0000_7FFF_FFFF_F000;
pub const USER_HEAP_START: u64 = 0x0000_0001_0000_0000;

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; 512],
}

#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    pub const fn unused() -> Self {
        PageTableEntry(0)
    }

    pub fn set_frame(&mut self, phys: u64, flags: PageFlags) {
        self.0 = (phys & 0x000F_FFFF_FFFF_F000) | flags.bits();
    }

    pub fn frame(&self) -> u64 {
        self.0 & 0x000F_FFFF_FFFF_F000
    }

    pub fn flags(&self) -> PageFlags {
        PageFlags::from_bits_truncate(self.0)
    }

    pub fn is_present(&self) -> bool {
        self.flags().contains(PageFlags::PRESENT)
    }

    pub fn is_huge(&self) -> bool {
        self.flags().contains(PageFlags::HUGE)
    }
}

pub fn get_cr3() -> u64 {
    let cr3: u64;
    unsafe { core::arch::asm!("mov {}, cr3", out(reg) cr3) }
    cr3 & !0xFFF
}

pub fn set_cr3(phys: u64) {
    unsafe { core::arch::asm!("mov cr3, {}", in(reg) phys, options(nomem)) }
}

pub fn invalidate_tlb(virt: u64) {
    unsafe { core::arch::asm!("invlpg [{0}]", in(reg) virt, options(nomem)) }
}

pub fn flush_tlb_all() {
    let cr3 = get_cr3();
    set_cr3(cr3);
}

pub struct PageMapper {
    pub pml4_phys: u64,
}

impl PageMapper {
    pub fn current() -> Self {
        PageMapper { pml4_phys: get_cr3() }
    }

    pub fn new(pml4_phys: u64) -> Self {
        PageMapper { pml4_phys }
    }

    pub fn activate(&self) {
        set_cr3(self.pml4_phys);
    }

    pub unsafe fn map_page(&mut self, virt: u64, phys: u64, flags: PageFlags) {
        let pml4 = &mut *(phys_to_virt(self.pml4_phys) as *mut PageTable);
        let pml4_idx = (virt >> 39) & 0x1FF;
        let pdpt_idx = (virt >> 30) & 0x1FF;
        let pd_idx   = (virt >> 21) & 0x1FF;
        let pt_idx   = (virt >> 12) & 0x1FF;

        let pdpt = ensure_table(&mut pml4.entries[pml4_idx as usize], flags);
        let pd = ensure_table(&mut pdpt.entries[pdpt_idx as usize], flags);
        let pt = ensure_table(&mut pd.entries[pd_idx as usize], flags);

        pt.entries[pt_idx as usize].set_frame(phys, flags | PageFlags::PRESENT);
        invalidate_tlb(virt);
    }

    pub unsafe fn unmap_page(&mut self, virt: u64) {
        if let Some(entry) = self.get_entry(virt) {
            *entry = PageTableEntry::unused();
            invalidate_tlb(virt);
        }
    }

    pub unsafe fn get_flags(&self, virt: u64) -> Option<PageFlags> {
        let entry = self.get_entry(virt)?;
        if (*entry).is_present() { Some((*entry).flags()) } else { None }
    }

    pub unsafe fn map_page_raw(&mut self, virt: u64, phys: u64, raw_flags: u64) {
        // Like map_page but accepts raw u64 flags (for embedding pkey bits)
        let pml4 = &mut *(phys_to_virt(self.pml4_phys) as *mut PageTable);
        let pml4_idx = (virt >> 39) & 0x1FF;
        let pdpt_idx = (virt >> 30) & 0x1FF;
        let pd_idx   = (virt >> 21) & 0x1FF;
        let pt_idx   = (virt >> 12) & 0x1FF;
        let flags = PageFlags::from_bits_truncate(raw_flags) | PageFlags::PRESENT | PageFlags::USER;
        let pdpt = ensure_table(&mut pml4.entries[pml4_idx as usize], flags);
        let pd   = ensure_table(&mut pdpt.entries[pdpt_idx as usize], flags);
        let pt   = ensure_table(&mut pd.entries[pd_idx as usize],   flags);
        pt.entries[pt_idx as usize].0 = (phys & 0x000F_FFFF_FFFF_F000) | raw_flags | PageFlags::PRESENT.bits();
        invalidate_tlb(virt);
    }

    pub unsafe fn translate(&self, virt: u64) -> Option<u64> {
        let entry = self.get_entry(virt)?;
        if unsafe { (*entry).is_present() } {
            Some(unsafe { (*entry).frame() } | (virt & 0xFFF))
        } else {
            None
        }
    }

    unsafe fn get_entry(&self, virt: u64) -> Option<*mut PageTableEntry> {
        let pml4 = &*(phys_to_virt(self.pml4_phys) as *const PageTable);
        let e4 = &pml4.entries[(virt >> 39 & 0x1FF) as usize];
        if !e4.is_present() { return None; }

        let pdpt = &*(phys_to_virt(e4.frame()) as *const PageTable);
        let e3 = &pdpt.entries[(virt >> 30 & 0x1FF) as usize];
        if !e3.is_present() { return None; }

        let pd = &*(phys_to_virt(e3.frame()) as *const PageTable);
        let e2 = &pd.entries[(virt >> 21 & 0x1FF) as usize];
        if !e2.is_present() { return None; }

        let pt = &*(phys_to_virt(e2.frame()) as *const PageTable);
        let e1 = &pt.entries[(virt >> 12 & 0x1FF) as usize] as *const _ as *mut _;
        Some(e1)
    }
}

unsafe fn ensure_table(entry: &mut PageTableEntry, flags: PageFlags) -> &mut PageTable {
    if !entry.is_present() {
        let frame = alloc_frame().expect("OOM in page table alloc");
        let virt = phys_to_virt(frame);
        core::ptr::write_bytes(virt as *mut u8, 0, PAGE_SIZE as usize);
        entry.set_frame(frame, flags | PageFlags::PRESENT | PageFlags::WRITABLE);
    }
    &mut *(phys_to_virt(entry.frame()) as *mut PageTable)
}

pub fn phys_to_virt(phys: u64) -> u64 {
    phys + KERNEL_VIRT_OFFSET
}

pub fn virt_to_phys(virt: u64) -> u64 {
    virt - KERNEL_VIRT_OFFSET
}
