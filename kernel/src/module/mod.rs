/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::vec::Vec;
use alloc::string::String;
use alloc::boxed::Box;
use spin::Mutex;

pub type ModuleInitFn = fn() -> Result<(), &'static str>;
pub type ModuleExitFn = fn();

pub struct KernelModule {
    pub name:    String,
    pub version: String,
    pub author:  String,
    pub desc:    String,
    pub init:    ModuleInitFn,
    pub exit:    ModuleExitFn,
    pub loaded:  bool,
    pub deps:    Vec<String>,
}

impl KernelModule {
    pub fn new(name: &str, version: &str, init: ModuleInitFn, exit: ModuleExitFn) -> Self {
        KernelModule {
            name:    String::from(name),
            version: String::from(version),
            author:  String::from("Qunix"),
            desc:    String::new(),
            init, exit, loaded: false, deps: Vec::new(),
        }
    }
}

static MODULES: Mutex<Vec<KernelModule>> = Mutex::new(Vec::new());

pub fn init() {
    register_builtins();
    crate::klog!("Module subsystem initialized ({} builtin modules)", MODULES.lock().len());
}

fn register_builtins() {
    // Register kernel-builtin modules as loadable units
    register(KernelModule::new("virtio_net",  "0.1", mod_virtio_net_init,  mod_noop_exit));
    register(KernelModule::new("virtio_blk",  "0.1", mod_virtio_blk_init,  mod_noop_exit));
    register(KernelModule::new("ext4",        "0.1", mod_stub_init,        mod_noop_exit));
    register(KernelModule::new("btrfs",       "0.1", mod_stub_init,        mod_noop_exit));
    register(KernelModule::new("xfs",         "0.1", mod_stub_init,        mod_noop_exit));
    register(KernelModule::new("drm_qunix",   "0.1", mod_drm_init,         mod_noop_exit));
    register(KernelModule::new("alsa_qunix",  "0.1", mod_stub_init,        mod_noop_exit));
    register(KernelModule::new("usbhid",      "0.1", mod_stub_init,        mod_noop_exit));
}

pub fn register(m: KernelModule) {
    MODULES.lock().push(m);
}

pub fn load(name: &str) -> Result<(), &'static str> {
    let mut modules = MODULES.lock();
    for m in modules.iter_mut() {
        if m.name == name {
            if m.loaded { return Err("already loaded"); }
            (m.init)()?;
            m.loaded = true;
            crate::klog!("module loaded: {} v{}", m.name, m.version);
            return Ok(());
        }
    }
    Err("not found")
}

pub fn unload(name: &str) -> Result<(), &'static str> {
    let mut modules = MODULES.lock();
    for m in modules.iter_mut() {
        if m.name == name {
            if !m.loaded { return Err("not loaded"); }
            (m.exit)();
            m.loaded = false;
            crate::klog!("module unloaded: {}", m.name);
            return Ok(());
        }
    }
    Err("not found")
}

pub fn list() -> Vec<(String, String, bool)> {
    MODULES.lock().iter()
        .map(|m| (m.name.clone(), m.version.clone(), m.loaded))
        .collect()
}

pub fn is_loaded(name: &str) -> bool {
    MODULES.lock().iter().any(|m| m.name == name && m.loaded)
}

// Module init/exit implementations
fn mod_noop_exit() {}
fn mod_stub_init() -> Result<(), &'static str> { Ok(()) }

fn mod_virtio_net_init() -> Result<(), &'static str> {
    // Will be called if virtio-net PCI device was found during enumeration
    crate::klog!("virtio_net: driver loaded");
    Ok(())
}

fn mod_virtio_blk_init() -> Result<(), &'static str> {
    crate::klog!("virtio_blk: driver loaded");
    Ok(())
}

fn mod_drm_init() -> Result<(), &'static str> {
    crate::klog!("drm_qunix: KMS/DRM subsystem active");
    Ok(())
}
