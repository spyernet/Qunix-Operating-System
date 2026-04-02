//! PCIe bus enumeration — all buses 0-255, all devices, all functions.
//! Supports PCI configuration space access via CAM (legacy) and ECAM (PCIe).
//! ECAM base address read from ACPI MCFG table.

use alloc::vec::Vec;
use spin::Mutex;
use crate::arch::x86_64::port::{inl, outl};

const PCI_CONFIG_ADDRESS: u16 = 0xCF8;
const PCI_CONFIG_DATA:    u16 = 0xCFC;

// ECAM (PCIe extended config) base - set by ACPI init
static ECAM_BASE: Mutex<u64> = Mutex::new(0);

pub fn set_ecam_base(base: u64) { *ECAM_BASE.lock() = base; }

#[derive(Clone, Debug)]
pub struct PciDevice {
    pub bus:       u8,
    pub device:    u8,
    pub function:  u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class:     u8,
    pub subclass:  u8,
    pub prog_if:   u8,
    pub revision:  u8,
    pub header:    u8,   // header type
    pub bars:      [u32; 6],
    pub irq_line:  u8,
    pub irq_pin:   u8,
    pub subsystem_vendor: u16,
    pub subsystem_id:     u16,
    pub capabilities_ptr: u8,
}

impl PciDevice {
    /// Return the 64-bit BAR address (handles 32/64-bit BARs).
    pub fn bar_addr(&self, idx: usize) -> u64 {
        if idx >= 6 { return 0; }
        let bar = self.bars[idx];
        if bar & 1 != 0 {
            // I/O space
            (bar & !3) as u64
        } else if (bar >> 1) & 3 == 2 {
            // 64-bit MMIO
            if idx + 1 < 6 {
                (bar & !0xF) as u64 | ((self.bars[idx + 1] as u64) << 32)
            } else {
                (bar & !0xF) as u64
            }
        } else {
            // 32-bit MMIO
            (bar & !0xF) as u64
        }
    }

    pub fn is_io_bar(&self, idx: usize) -> bool {
        if idx >= 6 { return false; }
        self.bars[idx] & 1 != 0
    }

    pub fn is_mmio_bar(&self, idx: usize) -> bool { !self.is_io_bar(idx) }

    /// Enable bus mastering and memory/IO decoding.
    pub fn enable(&self) {
        let cmd = pci_read_u16(self.bus, self.device, self.function, 0x04);
        // Bit 0: I/O space, Bit 1: Memory space, Bit 2: Bus Master
        pci_write_u16(self.bus, self.device, self.function, 0x04, cmd | 0x7);
    }

    /// Read a capability from the PCI capability list.
    pub fn find_capability(&self, cap_id: u8) -> Option<u8> {
        if self.capabilities_ptr == 0 { return None; }
        let mut ptr = self.capabilities_ptr;
        for _ in 0..48 { // max 48 capabilities
            if ptr == 0 { break; }
            let id   = pci_read_u8(self.bus, self.device, self.function, ptr);
            let next = pci_read_u8(self.bus, self.device, self.function, ptr + 1);
            if id == cap_id { return Some(ptr); }
            ptr = next;
        }
        None
    }
}

// ── PCI config space access ───────────────────────────────────────────────

fn pci_addr(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    0x8000_0000
        | ((bus  as u32) << 16)
        | ((dev  as u32) << 11)
        | ((func as u32) << 8)
        | ((off  as u32) & 0xFC)
}

pub fn pci_read_u32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    // Use ECAM if available, else legacy CAM
    let ecam = *ECAM_BASE.lock();
    if ecam != 0 {
        let addr = ecam
            + ((bus  as u64) << 20)
            + ((dev  as u64) << 15)
            + ((func as u64) << 12)
            + (off   as u64 & !3);
        let virt = addr + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
        unsafe { core::ptr::read_volatile(virt as *const u32) }
    } else {
        unsafe {
            outl(PCI_CONFIG_ADDRESS, pci_addr(bus, dev, func, off));
            inl(PCI_CONFIG_DATA)
        }
    }
}

pub fn pci_write_u32(bus: u8, dev: u8, func: u8, off: u8, val: u32) {
    let ecam = *ECAM_BASE.lock();
    if ecam != 0 {
        let addr = ecam
            + ((bus  as u64) << 20)
            + ((dev  as u64) << 15)
            + ((func as u64) << 12)
            + (off   as u64 & !3);
        let virt = addr + crate::arch::x86_64::paging::KERNEL_VIRT_OFFSET;
        unsafe { core::ptr::write_volatile(virt as *mut u32, val); }
    } else {
        unsafe {
            outl(PCI_CONFIG_ADDRESS, pci_addr(bus, dev, func, off));
            outl(PCI_CONFIG_DATA, val);
        }
    }
}

pub fn pci_read_u16(bus: u8, dev: u8, func: u8, off: u8) -> u16 {
    let d = pci_read_u32(bus, dev, func, off & !3);
    if off & 2 != 0 { (d >> 16) as u16 } else { d as u16 }
}

pub fn pci_write_u16(bus: u8, dev: u8, func: u8, off: u8, val: u16) {
    let d = pci_read_u32(bus, dev, func, off & !3);
    let new = if off & 2 != 0 {
        (d & 0x0000_FFFF) | ((val as u32) << 16)
    } else {
        (d & 0xFFFF_0000) | val as u32
    };
    pci_write_u32(bus, dev, func, off & !3, new);
}

pub fn pci_read_u8(bus: u8, dev: u8, func: u8, off: u8) -> u8 {
    let d = pci_read_u32(bus, dev, func, off & !3);
    (d >> ((off & 3) * 8)) as u8
}

// ── Full enumeration ──────────────────────────────────────────────────────

static DEVICES: Mutex<Vec<PciDevice>> = Mutex::new(Vec::new());

pub fn init() {
    let mut devs = Vec::new();
    // Scan all 256 buses, 32 devices, 8 functions
    // For speed: first check bus 0, then follow P2P bridges
    scan_bus(0, &mut devs);

    let count = devs.len();
    for dev in &devs {
        crate::klog!("PCI {:02x}:{:02x}.{} {:04x}:{:04x} class={:02x}/{:02x} bars=[{:#x},{:#x}]",
            dev.bus, dev.device, dev.function,
            dev.vendor_id, dev.device_id,
            dev.class, dev.subclass,
            dev.bar_addr(0), dev.bar_addr(1));

        handle_device(dev);
    }

    *DEVICES.lock() = devs;
    crate::klog!("PCI: {} devices found", count);
}

fn scan_bus(bus: u8, devs: &mut Vec<PciDevice>) {
    for device in 0u8..32 {
        let vendor = pci_read_u16(bus, device, 0, 0x00);
        if vendor == 0xFFFF { continue; }

        let header = pci_read_u8(bus, device, 0, 0x0E);
        let nfuncs = if header & 0x80 != 0 { 8 } else { 1 };

        for function in 0..nfuncs {
            let vendor = pci_read_u16(bus, device, function, 0x00);
            if vendor == 0xFFFF { continue; }

            let dev = read_device(bus, device, function);

            // Recursively scan PCI-to-PCI bridges
            if dev.class == 0x06 && dev.subclass == 0x04 {
                let secondary_bus = pci_read_u8(bus, device, function, 0x19);
                if secondary_bus != 0 && secondary_bus != bus {
                    scan_bus(secondary_bus, devs);
                }
            }

            devs.push(dev);
        }
    }
}

fn read_device(bus: u8, device: u8, function: u8) -> PciDevice {
    let vendor_device = pci_read_u32(bus, device, function, 0x00);
    let class_rev     = pci_read_u32(bus, device, function, 0x08);
    let header_etc    = pci_read_u32(bus, device, function, 0x0C);
    let irq_info      = pci_read_u32(bus, device, function, 0x3C);
    let subsys        = pci_read_u32(bus, device, function, 0x2C);
    let status_cmd    = pci_read_u32(bus, device, function, 0x04);
    let caps_ptr      = if (status_cmd >> 16) & 0x10 != 0 {
        pci_read_u8(bus, device, function, 0x34) & !3
    } else { 0 };

    let mut bars = [0u32; 6];
    let header_type = ((header_etc >> 16) & 0xFF) as u8;
    let n_bars = if header_type & 0x7F == 0 { 6 } else { 2 };
    for i in 0..n_bars {
        bars[i] = pci_read_u32(bus, device, function, 0x10 + (i as u8) * 4);
    }

    PciDevice {
        bus, device, function,
        vendor_id:  (vendor_device & 0xFFFF) as u16,
        device_id:  (vendor_device >> 16) as u16,
        class:      (class_rev >> 24) as u8,
        subclass:   ((class_rev >> 16) & 0xFF) as u8,
        prog_if:    ((class_rev >> 8) & 0xFF) as u8,
        revision:   (class_rev & 0xFF) as u8,
        header:     header_type,
        bars,
        irq_line:   (irq_info & 0xFF) as u8,
        irq_pin:    ((irq_info >> 8) & 0xFF) as u8,
        subsystem_vendor: (subsys & 0xFFFF) as u16,
        subsystem_id:     (subsys >> 16) as u16,
        capabilities_ptr: caps_ptr,
    }
}

fn handle_device(dev: &PciDevice) {
    dev.enable(); // always enable bus mastering

    match (dev.class, dev.subclass, dev.prog_if) {
        // NVMe (Mass Storage, NVM, NVMe)
        (0x01, 0x08, 0x02) => {
            let bar0 = dev.bar_addr(0);
            if bar0 != 0 { crate::drivers::nvme::register_device(bar0); }
        }
        // AHCI SATA
        (0x01, 0x06, 0x01) => {
            let bar5 = dev.bar_addr(5);
            crate::klog!("PCI: AHCI SATA at BAR5={:#x}", bar5);
            // TODO: AHCI driver
        }
        // VirtIO net (legacy)
        (0x02, 0x00, _) if dev.vendor_id == 0x1AF4 && dev.device_id == 0x1000 => {
            // virtio-net handled by drivers/net.rs
        }
        // VirtIO block
        (0x01, 0x00, _) if dev.vendor_id == 0x1AF4 && dev.device_id == 0x1001 => {
            crate::klog!("PCI: virtio-blk found");
        }
        // Intel/AMD GPU
        (0x03, _, _) => {
            crate::klog!("PCI: GPU {:04x}:{:04x}", dev.vendor_id, dev.device_id);
        }
        // USB xHCI
        (0x0C, 0x03, 0x30) => {
            crate::klog!("PCI: xHCI USB controller");
        }
        // USB EHCI
        (0x0C, 0x03, 0x20) => {
            crate::klog!("PCI: EHCI USB controller");
        }
        // Intel HDA audio
        (0x04, 0x03, _) => {
            crate::klog!("PCI: HDA audio {:04x}:{:04x}", dev.vendor_id, dev.device_id);
        }
        // Network (Ethernet) — various
        (0x02, 0x00, _) => {
            crate::klog!("PCI: NIC {:04x}:{:04x}", dev.vendor_id, dev.device_id);
        }
        _ => {}
    }
}

// ── Public query API ──────────────────────────────────────────────────────

pub fn find_device(vendor: u16, device: u16) -> Option<PciDevice> {
    DEVICES.lock().iter()
        .find(|d| d.vendor_id == vendor && d.device_id == device)
        .cloned()
}

pub fn find_class(class: u8, subclass: u8) -> Vec<PciDevice> {
    DEVICES.lock().iter()
        .filter(|d| d.class == class && d.subclass == subclass)
        .cloned()
        .collect()
}

pub fn all_devices() -> Vec<PciDevice> { DEVICES.lock().clone() }

pub fn enable_bus_mastering(dev: &PciDevice) { dev.enable(); }
