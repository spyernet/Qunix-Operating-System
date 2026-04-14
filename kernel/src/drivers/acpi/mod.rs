/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use crate::arch::x86_64::paging::phys_to_virt;

const RSDP_SIG:  &[u8; 8] = b"RSD PTR ";
const RSDT_SIG:  &[u8; 4] = b"RSDT";
const XSDT_SIG:  &[u8; 4] = b"XSDT";
const MADT_SIG:  &[u8; 4] = b"APIC";
const FADT_SIG:  &[u8; 4] = b"FACP";
const HPET_SIG:  &[u8; 4] = b"HPET";

#[repr(C, packed)]
struct Rsdp {
    signature: [u8; 8],
    checksum:  u8,
    oem_id:    [u8; 6],
    revision:  u8,
    rsdt_addr: u32,
    length:    u32,
    xsdt_addr: u64,
    ext_chk:   u8,
    _reserved: [u8; 3],
}

#[repr(C, packed)]
struct AcpiHeader {
    signature:  [u8; 4],
    length:     u32,
    revision:   u8,
    checksum:   u8,
    oem_id:     [u8; 6],
    oem_table:  [u8; 8],
    oem_rev:    u32,
    creator_id: u32,
    creator_rev: u32,
}

#[repr(C, packed)]
struct MadtEntry {
    entry_type: u8,
    length:     u8,
}

#[repr(C, packed)]
struct MadtLapic {
    header:      MadtEntry,
    acpi_id:     u8,
    apic_id:     u8,
    flags:       u32,
}

#[repr(C, packed)]
struct MadtIoapic {
    header:      MadtEntry,
    ioapic_id:   u8,
    _reserved:   u8,
    ioapic_addr: u32,
    gsi_base:    u32,
}

pub struct AcpiInfo {
    pub lapic_addr:   u64,
    pub ioapic_addr:  u64,
    pub cpu_count:    usize,
    pub apic_ids:     [u8; 32],
    pub hpet_addr:    u64,
    pub pm_timer_blk: u32,
}

static mut ACPI_INFO: AcpiInfo = AcpiInfo {
    lapic_addr: 0xFEE0_0000,
    ioapic_addr: 0xFEC0_0000,
    cpu_count: 1,
    apic_ids: [0; 32],
    hpet_addr: 0,
    pm_timer_blk: 0,
};

pub fn init(rsdp_phys: u64) {
    if rsdp_phys == 0 {
        crate::klog!("ACPI: No RSDP — using defaults");
        return;
    }

    let rsdp = unsafe { &*(phys_to_virt(rsdp_phys) as *const Rsdp) };
    if &rsdp.signature != RSDP_SIG {
        crate::klog!("ACPI: Invalid RSDP signature");
        return;
    }

    crate::klog!("ACPI: RSDP rev={}", rsdp.revision);

    let (table_base, use_xsdt, entry_size) = if rsdp.revision >= 2 && rsdp.xsdt_addr != 0 {
        (rsdp.xsdt_addr, true, 8usize)
    } else {
        (rsdp.rsdt_addr as u64, false, 4usize)
    };

    let root_hdr = unsafe { &*(phys_to_virt(table_base) as *const AcpiHeader) };
    let hdr_len  = core::mem::size_of::<AcpiHeader>();
    let entries_len = (u32::from_le(root_hdr.length) as usize).saturating_sub(hdr_len);
    let entries_ptr = phys_to_virt(table_base) as usize + hdr_len;
    let n_entries   = entries_len / entry_size;

    for i in 0..n_entries {
        let entry_phys: u64 = if use_xsdt {
            unsafe { *(((entries_ptr + i * 8) as *const u64)) }
        } else {
            unsafe { *(((entries_ptr + i * 4) as *const u32)) as u64 }
        };
        parse_table(entry_phys);
    }

    unsafe {
        crate::klog!(
            "ACPI: {} CPUs, LAPIC={:#x} IOAPIC={:#x} HPET={:#x}",
            ACPI_INFO.cpu_count, ACPI_INFO.lapic_addr,
            ACPI_INFO.ioapic_addr, ACPI_INFO.hpet_addr
        );
    }
}

fn parse_table(phys: u64) {
    if phys == 0 { return; }
    let virt = phys_to_virt(phys);
    let hdr  = unsafe { &*(virt as *const AcpiHeader) };
    match &hdr.signature {
        s if s == MADT_SIG => parse_madt(virt, u32::from_le(hdr.length) as usize),
        s if s == HPET_SIG => parse_hpet(virt),
        s if s == FADT_SIG => parse_fadt(virt),
        _ => {}
    }
}

fn parse_madt(virt: u64, len: usize) {
    let hdr_size = core::mem::size_of::<AcpiHeader>() + 8; // +local_apic_addr+flags
    let base     = virt as usize + hdr_size;

    // Read local APIC address
    let lapic_addr = unsafe { *(( virt as usize + core::mem::size_of::<AcpiHeader>()) as *const u32) } as u64;
    unsafe { ACPI_INFO.lapic_addr = lapic_addr; }

    let mut off = base;
    let end     = virt as usize + len;
    let mut cpu_idx = 0usize;

    while off + 2 <= end {
        let entry = unsafe { &*(off as *const MadtEntry) };
        let elen  = entry.length as usize;
        if elen < 2 || off + elen > end { break; }

        match entry.entry_type {
            0 => {
                let lapic = unsafe { &*(off as *const MadtLapic) };
                if u32::from_le(lapic.flags) & 1 != 0 && cpu_idx < 32 {
                    unsafe { ACPI_INFO.apic_ids[cpu_idx] = lapic.apic_id; }
                    cpu_idx += 1;
                }
            }
            1 => {
                let ioapic = unsafe { &*(off as *const MadtIoapic) };
                unsafe { ACPI_INFO.ioapic_addr = u32::from_le(ioapic.ioapic_addr) as u64; }
            }
            _ => {}
        }
        off += elen;
    }

    unsafe { ACPI_INFO.cpu_count = cpu_idx.max(1); }
}

fn parse_hpet(virt: u64) {
    // HPET address is at offset 44 in the table
    let addr_off = virt as usize + core::mem::size_of::<AcpiHeader>() + 4 + 1;
    let hpet_addr = unsafe { *(addr_off as *const u64) };
    unsafe { ACPI_INFO.hpet_addr = hpet_addr; }
}

fn parse_fadt(virt: u64) {
    // PM timer block at offset 116
    let pm_off = virt as usize + core::mem::size_of::<AcpiHeader>() + 64;
    let pm_blk = unsafe { *(pm_off as *const u32) };
    unsafe { ACPI_INFO.pm_timer_blk = u32::from_le(pm_blk); }
}

pub fn cpu_count() -> usize  { unsafe { ACPI_INFO.cpu_count } }
pub fn lapic_addr() -> u64   { unsafe { ACPI_INFO.lapic_addr } }
pub fn ioapic_addr() -> u64  { unsafe { ACPI_INFO.ioapic_addr } }
pub fn hpet_addr() -> u64    { unsafe { ACPI_INFO.hpet_addr } }

pub fn acpi_poweroff() {
    // PM1a control – SLP_TYP=5, SLP_EN=1 for S5
    unsafe { crate::arch::x86_64::port::outw(0x604, 0x2000); } // QEMU ACPI poweroff
    unsafe { crate::arch::x86_64::port::outw(0xB004, 0x2000); } // bochs/older QEMU
    unsafe { crate::arch::x86_64::port::outw(0x4004, 0x3400); } // VirtualBox
}

pub fn acpi_reboot() {
    unsafe { crate::arch::x86_64::port::outb(0x64, 0xFE); } // PS/2 reset
}
