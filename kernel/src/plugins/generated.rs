//! Auto-generated plugin registry — DO NOT EDIT
//! Plugins compiled in: 2

mod perf_monitor {
    include!("../../../plugins/perf_monitor/plug/main.rs");
}
mod syscall_logger {
    include!("../../../plugins/syscall_logger/plug/main.rs");
}

/// Register all compiled-in plugins with the kernel plugin manager.
pub fn register_all() {

    crate::plugins::register(&perf_monitor::PLUGIN_ENTRY);
    crate::plugins::register(&syscall_logger::PLUGIN_ENTRY);

    let enabled_count = crate::plugins::list()
        .iter()
        .filter(|(_name, enabled, _ver, _desc)| *enabled)
        .count() as u32;
    crate::plugins::hooks::HOOKS_ACTIVE.store(
        enabled_count,
        core::sync::atomic::Ordering::Relaxed,
    );
}
