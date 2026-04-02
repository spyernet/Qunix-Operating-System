//! pluginctl — Qunix kernel plugin control utility.
//!
//! Usage:
//!   pluginctl list                    List all compiled-in plugins
//!   pluginctl enable  <name>          Enable a plugin at runtime
//!   pluginctl disable <name>          Disable a plugin at runtime
//!   pluginctl info    <name>          Show plugin details
//!   pluginctl status                  Show overall plugin system status
//!
//! Communication:
//!   Opens /dev/pluginctl and issues ioctl() commands.
//!   All state changes are reflected in /proc/plugins instantly.
//!   No kernel rebuild required.

#![no_std]
#![no_main]
extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use libsys::*;

// ioctl numbers must match device/mod.rs
const IOCTL_PLUGIN_ENABLE:  u64 = 0x5100;
const IOCTL_PLUGIN_DISABLE: u64 = 0x5101;
const IOCTL_PLUGIN_LIST:    u64 = 0x5102;

fn wstr(s: &str) { write(STDOUT, s.as_bytes()); }
fn werr(s: &str) { write(STDERR, s.as_bytes()); }

fn args_from_argv(argc: u64, argv: *const *const u8) -> Vec<String> {
    (0..argc as usize).map(|i| unsafe {
        let p = *argv.add(i);
        let mut len = 0;
        while *p.add(len) != 0 { len += 1; }
        String::from_utf8_lossy(core::slice::from_raw_parts(p, len)).to_string()
    }).collect()
}

fn open_pluginctl() -> i32 {
    let fd = open(b"/dev/pluginctl\0", O_RDWR, 0);
    if fd < 0 {
        werr("pluginctl: cannot open /dev/pluginctl — is the kernel compiled with plugin support?\n");
        exit(1);
    }
    fd as i32
}

/// Read /proc/plugins and return its content.
fn read_proc_plugins() -> String {
    let fd = open(b"/proc/plugins\0", O_RDONLY, 0);
    if fd < 0 { return String::from("(cannot read /proc/plugins)\n"); }
    let mut buf = alloc::vec![0u8; 4096];
    let n = read(fd as i32, &mut buf);
    close(fd as i32);
    if n > 0 { String::from_utf8_lossy(&buf[..n as usize]).to_string() }
    else { String::from("(empty)\n") }
}

#[no_mangle]
#[link_section = ".text._start"]
pub extern "C" fn _start(argc: u64, argv: *const *const u8) -> ! {
    let args = args_from_argv(argc, argv);

    if args.len() < 2 {
        print_usage();
        exit(1);
    }

    match args[1].as_str() {
        "list" | "ls" => { cmd_list(); exit(0); }
        "status"      => { cmd_status(); exit(0); }
        "enable"  if args.len() >= 3 => { cmd_enable(&args[2]); exit(0); }
        "disable" if args.len() >= 3 => { cmd_disable(&args[2]); exit(0); }
        "info"    if args.len() >= 3 => { cmd_info(&args[2]); exit(0); }
        "help" | "--help" | "-h" => { print_usage(); exit(0); }
        "enable" | "disable" | "info" => {
            werr("pluginctl: missing plugin name\n");
            werr("Usage: pluginctl enable|disable|info <name>\n");
            exit(1);
        }
        cmd => {
            werr(&alloc::format!("pluginctl: unknown command '{}'\n", cmd));
            print_usage();
            exit(1);
        }
    }
}

fn print_usage() {
    wstr("pluginctl — Qunix Plugin Control\n\n");
    wstr("Usage:\n");
    wstr("  pluginctl list              List all plugins and their state\n");
    wstr("  pluginctl enable  <name>    Enable a plugin at runtime\n");
    wstr("  pluginctl disable <name>    Disable a plugin at runtime\n");
    wstr("  pluginctl info    <name>    Show details for a plugin\n");
    wstr("  pluginctl status            Show plugin system summary\n\n");
    wstr("Plugins communicate via /dev/pluginctl and /proc/plugins.\n");
    wstr("No kernel rebuild required to enable/disable.\n");
}

fn cmd_list() {
    let content = read_proc_plugins();
    // Parse and format nicely
    wstr("NAME                 VERSION  STATE     DESCRIPTION\n");
    wstr("─────────────────────────────────────────────────────────────────\n");

    for line in content.lines() {
        if line.starts_with('#') || line.trim().is_empty() { continue; }
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        if parts.len() < 3 { continue; }
        let name    = parts[0];
        let version = parts[1];
        let state   = parts[2];
        let desc    = if parts.len() > 3 { parts[3] } else { "" };
        let state_colored = if state == "enabled" {
            "\x1b[32menabled \x1b[0m"
        } else {
            "\x1b[33mdisabled\x1b[0m"
        };
        wstr(&alloc::format!("{:<20} {:<8} {}  {}\n", name, version, state_colored, desc));
    }
}

fn cmd_status() {
    let content = read_proc_plugins();
    let total   = content.lines().filter(|l| !l.starts_with('#') && !l.trim().is_empty()).count();
    let enabled = content.lines().filter(|l| l.contains("enabled") && !l.starts_with('#')).count();
    let disabled = total.saturating_sub(enabled);

    wstr("\x1b[1mQunix Plugin System Status\x1b[0m\n");
    wstr(&alloc::format!("  Total plugins:    {}\n", total));
    wstr(&alloc::format!("  Enabled:          {}\n", enabled));
    wstr(&alloc::format!("  Disabled:         {}\n", disabled));
    wstr("\n");
    wstr("Plugin state source: /proc/plugins\n");
    wstr("Control interface:   /dev/pluginctl\n");
    wstr("Note: Adding/removing plugins requires kernel rebuild.\n");
    wstr("      Enabling/disabling does NOT require rebuild.\n");
}

fn cmd_enable(name: &str) {
    let fd = open_pluginctl();
    // Prepare NUL-terminated name
    let mut name_buf = name.as_bytes().to_vec();
    name_buf.push(0);
    let r = ioctl(fd, IOCTL_PLUGIN_ENABLE, name_buf.as_ptr() as u64);
    close(fd);
    if r == 0 {
        wstr(&alloc::format!("\x1b[32m✓\x1b[0m Plugin '{}' enabled.\n", name));
    } else {
        werr(&alloc::format!("pluginctl: plugin '{}' not found (is it compiled in?)\n", name));
        werr("Available plugins:\n");
        cmd_list();
        exit(1);
    }
}

fn cmd_disable(name: &str) {
    let fd = open_pluginctl();
    let mut name_buf = name.as_bytes().to_vec();
    name_buf.push(0);
    let r = ioctl(fd, IOCTL_PLUGIN_DISABLE, name_buf.as_ptr() as u64);
    close(fd);
    if r == 0 {
        wstr(&alloc::format!("\x1b[33m○\x1b[0m Plugin '{}' disabled.\n", name));
    } else {
        werr(&alloc::format!("pluginctl: plugin '{}' not found\n", name));
        exit(1);
    }
}

fn cmd_info(name: &str) {
    let content = read_proc_plugins();
    for line in content.lines() {
        if line.starts_with('#') || line.trim().is_empty() { continue; }
        let parts: Vec<&str> = line.splitn(4, ' ').collect();
        if parts.len() < 3 { continue; }
        if parts[0] == name {
            let state = if parts[2] == "enabled" { "\x1b[32menabled\x1b[0m" } else { "\x1b[33mdisabled\x1b[0m" };
            wstr(&alloc::format!("Plugin:      {}\n", parts[0]));
            wstr(&alloc::format!("Version:     {}\n", parts[1]));
            wstr(&alloc::format!("State:       {}\n", state));
            if parts.len() > 3 { wstr(&alloc::format!("Description: {}\n", parts[3])); }
            wstr("\n");
            wstr("To toggle: pluginctl enable/disable ");
            wstr(name);
            wstr("\n");
            exit(0);
        }
    }
    werr(&alloc::format!("pluginctl: plugin '{}' not found\n", name));
    exit(1);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    write(STDERR, b"pluginctl: internal error\n");
    exit(1)
}
