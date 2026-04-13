//! Kernel logger — writes to serial port, VGA text mode, and framebuffer console.

use core::fmt;
use core::panic::PanicInfo;

pub struct KernelLogger;

impl fmt::Write for KernelLogger {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // 1. Serial (always works, primary debug output)
        crate::drivers::serial::write_str(s);
        // 2. VGA text mode (works before framebuffer init)
        crate::drivers::vga::write_str(s);
        // 3. Framebuffer console (works after GPU init)
        if crate::drivers::gpu::console::is_ready() {
            crate::drivers::gpu::console::write_str(s);
        }
        Ok(())
    }
}

pub fn _klog(args: fmt::Arguments) {
    use fmt::Write;
    let _irq_guard = crate::arch::x86_64::cpu::IrqGuard::new();
    let mut l = KernelLogger;
    let _ = l.write_str("[QUNIX] ");
    let _ = l.write_fmt(args);
    let _ = l.write_str("\n");
}

pub fn _kprint(args: fmt::Arguments) {
    use fmt::Write;
    let _irq_guard = crate::arch::x86_64::cpu::IrqGuard::new();
    let _ = KernelLogger.write_fmt(args);
}

pub fn panic_handler(info: &PanicInfo) -> ! {
    use fmt::Write;
    let _irq_guard = crate::arch::x86_64::cpu::IrqGuard::new();
    let mut l = KernelLogger;
    let _ = writeln!(l, "\n--- QUNIX KERNEL PANIC ---");
    if let Some(loc) = info.location() {
        let _ = writeln!(l, "at {}:{}:{}", loc.file(), loc.line(), loc.column());
    }
    let msg = &info.message();
    let _ = writeln!(l, "msg: {}", msg);
    let _ = writeln!(l, "--------------------------");
    // Red screen of death on framebuffer
    if crate::drivers::gpu::console::is_ready() {
        crate::drivers::gpu::fill_rect(0, 0, 9999, 9999, 0x220000);
        crate::drivers::gpu::draw_str(10, 10, "KERNEL PANIC", 0xFF4444, 0x220000);
        if let Some(loc) = info.location() {
            // Format without alloc
            crate::drivers::gpu::draw_str(10, 30, loc.file(), 0xCCCCCC, 0x220000);
        }
    }
    unsafe { loop { core::arch::asm!("cli; hlt"); } }
}
