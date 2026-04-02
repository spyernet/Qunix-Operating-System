use core::sync::atomic::{AtomicU64, Ordering};

pub struct PerfCounters {
    pub context_switches:   AtomicU64,
    pub page_faults:        AtomicU64,
    pub syscalls:           AtomicU64,
    pub irqs:               AtomicU64,
    pub sched_yields:       AtomicU64,
    pub alloc_calls:        AtomicU64,
    pub free_calls:         AtomicU64,
    pub vfs_reads:          AtomicU64,
    pub vfs_writes:         AtomicU64,
    pub pipe_sends:         AtomicU64,
    pub signal_sends:       AtomicU64,
    pub abi_syscalls:       AtomicU64,
}

pub static PERF: PerfCounters = PerfCounters {
    context_switches: AtomicU64::new(0),
    page_faults:      AtomicU64::new(0),
    syscalls:         AtomicU64::new(0),
    irqs:             AtomicU64::new(0),
    sched_yields:     AtomicU64::new(0),
    alloc_calls:      AtomicU64::new(0),
    free_calls:       AtomicU64::new(0),
    vfs_reads:        AtomicU64::new(0),
    vfs_writes:       AtomicU64::new(0),
    pipe_sends:       AtomicU64::new(0),
    signal_sends:     AtomicU64::new(0),
    abi_syscalls:   AtomicU64::new(0),
};

impl PerfCounters {
    pub fn inc_ctx_switch(&self) { self.context_switches.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_page_fault(&self) { self.page_faults.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_syscall(&self)    { self.syscalls.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_irq(&self)        { self.irqs.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_abi_syscall(&self) { self.abi_syscalls.fetch_add(1, Ordering::Relaxed); }
}

pub fn snapshot() -> alloc::string::String {
    alloc::format!(
        "context_switches={}\npage_faults={}\nsyscalls={}\nirqs={}\n\
         vfs_reads={}\nvfs_writes={}\nabi_syscalls={}\n",
        PERF.context_switches.load(Ordering::Relaxed),
        PERF.page_faults.load(Ordering::Relaxed),
        PERF.syscalls.load(Ordering::Relaxed),
        PERF.irqs.load(Ordering::Relaxed),
        PERF.vfs_reads.load(Ordering::Relaxed),
        PERF.vfs_writes.load(Ordering::Relaxed),
        PERF.abi_syscalls.load(Ordering::Relaxed),
    )
}
