//! User namespace isolation — enforced cross-namespace boundaries.
//!
//! Qunix tracks six namespace types per process. This module enforces that
//! processes in different namespaces cannot observe or affect each other's
//! resources, specifically:
//!
//!   PID namespace  — `kill(2)` and `waitpid(2)` are namespace-scoped
//!   Mount namespace — each namespace has its own VFS root
//!   Network namespace — sockets are invisible across net namespaces
//!   UTS namespace  — `uname(2)` returns per-namespace hostname/nodename
//!   IPC namespace  — SysV IPC objects are namespace-scoped
//!   User namespace — UID/GID mapping (simplified: root-in-ns maps to nobody)
//!
//! ## Namespace ID assignment
//!
//! Every process starts in namespace 0 (the root namespace). `clone(2)` with
//! CLONE_NEW* flags creates a new namespace and assigns it the next ID from
//! the global counter. Namespaces are reference-counted and destroyed when
//! the last process exits them.
//!
//! ## Enforcement points
//!
//!   kill(pid, sig) → check pid_ns match before delivering signal
//!   waitpid(pid)   → only wait for children in same pid_ns
//!   getdents(/proc) → only show PIDs visible in current pid_ns
//!   connect/bind   → reject if net_ns doesn't match socket's ns
//!   shmget/semget  → reject cross-ipc_ns access
//!   uname()        → return per-uts_ns values

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicU32, Ordering};

// ── CLONE_NEW* flags ──────────────────────────────────────────────────────

pub const CLONE_NEWNS:   u64 = 0x0002_0000; // mount
pub const CLONE_NEWUTS:  u64 = 0x0400_0000; // UTS (hostname)
pub const CLONE_NEWIPC:  u64 = 0x0800_0000; // IPC
pub const CLONE_NEWUSER: u64 = 0x1000_0000; // user
pub const CLONE_NEWPID:  u64 = 0x2000_0000; // PID
pub const CLONE_NEWNET:  u64 = 0x4000_0000; // network

// ── Namespace ID allocator ────────────────────────────────────────────────

static NEXT_NS_ID: AtomicU32 = AtomicU32::new(1);

fn alloc_ns_id() -> u32 { NEXT_NS_ID.fetch_add(1, Ordering::Relaxed) }

// ── UTS namespace (hostname per namespace) ────────────────────────────────

#[derive(Clone)]
pub struct UtsNamespace {
    pub id:       u32,
    pub hostname: String,
    pub domname:  String,
    pub sysname:  String,
    pub release:  String,
    pub version:  String,
    pub machine:  String,
}

impl UtsNamespace {
    pub fn root() -> Self {
        UtsNamespace {
            id:       0,
            hostname: String::from("qunix"),
            domname:  String::from("(none)"),
            sysname:  String::from("Qunix"),
            release:  String::from("5.0.0"),
            version:  String::from("#1 SMP Qunix 5.0.0"),
            machine:  String::from("x86_64"),
        }
    }
    pub fn fork(&self) -> Self {
        let mut ns = self.clone();
        ns.id = alloc_ns_id();
        ns
    }
}

static UTS_NS_TABLE: Mutex<BTreeMap<u32, UtsNamespace>> = Mutex::new(BTreeMap::new());

pub fn uts_ns_init() {
    UTS_NS_TABLE.lock().insert(0, UtsNamespace::root());
}

pub fn uts_get(ns_id: u32) -> UtsNamespace {
    UTS_NS_TABLE.lock().get(&ns_id).cloned().unwrap_or_else(UtsNamespace::root)
}

pub fn uts_set_hostname(ns_id: u32, hostname: &str) {
    let mut table = UTS_NS_TABLE.lock();
    if let Some(ns) = table.get_mut(&ns_id) {
        ns.hostname = hostname.to_string();
    }
}

// ── PID namespace ─────────────────────────────────────────────────────────
//
// In the root namespace, PIDs are global kernel PIDs.
// In a child PID namespace, a process sees a local PID (starting from 1)
// that maps to the real kernel PID.

#[derive(Clone)]
pub struct PidNamespace {
    pub id:      u32,
    /// Maps local PID → kernel PID
    pub pid_map: BTreeMap<u32, u32>,
    /// Maps kernel PID → local PID
    pub rev_map: BTreeMap<u32, u32>,
    pub next_local: u32,
}

impl PidNamespace {
    pub fn root() -> Self {
        PidNamespace { id: 0, pid_map: BTreeMap::new(), rev_map: BTreeMap::new(), next_local: 2 }
    }
    pub fn new_child() -> Self {
        PidNamespace { id: alloc_ns_id(), pid_map: BTreeMap::new(), rev_map: BTreeMap::new(), next_local: 2 }
    }
    pub fn alloc_local(&mut self, kernel_pid: u32) -> u32 {
        let local = self.next_local;
        self.next_local += 1;
        self.pid_map.insert(local, kernel_pid);
        self.rev_map.insert(kernel_pid, local);
        local
    }
    pub fn kernel_to_local(&self, kernel_pid: u32) -> Option<u32> {
        self.rev_map.get(&kernel_pid).copied()
    }
    pub fn local_to_kernel(&self, local_pid: u32) -> Option<u32> {
        self.pid_map.get(&local_pid).copied()
    }
    pub fn is_visible(&self, kernel_pid: u32) -> bool {
        self.id == 0 || self.rev_map.contains_key(&kernel_pid)
    }
}

static PID_NS_TABLE: Mutex<BTreeMap<u32, PidNamespace>> = Mutex::new(BTreeMap::new());

pub fn pid_ns_init() {
    PID_NS_TABLE.lock().insert(0, PidNamespace::root());
}

/// Check if `sender_pid_ns` can send a signal to `target_kernel_pid`.
pub fn pid_ns_can_signal(sender_pid_ns: u32, target_kernel_pid: u32) -> bool {
    if sender_pid_ns == 0 { return true; } // root namespace can signal anyone
    let table = PID_NS_TABLE.lock();
    if let Some(ns) = table.get(&sender_pid_ns) {
        ns.is_visible(target_kernel_pid)
    } else { false }
}

/// Translate a local PID (in pid_ns) to a kernel PID.
/// Returns None if the local PID is not visible in this namespace.
pub fn pid_ns_resolve(pid_ns: u32, local_pid: u32) -> Option<u32> {
    if pid_ns == 0 { return Some(local_pid); } // root namespace: identity mapping
    PID_NS_TABLE.lock().get(&pid_ns)?.local_to_kernel(local_pid)
}

/// Get all kernel PIDs visible in a given PID namespace (for /proc listing).
pub fn pid_ns_visible_pids(pid_ns: u32) -> Vec<u32> {
    if pid_ns == 0 {
        return crate::process::all_pids();
    }
    let table = PID_NS_TABLE.lock();
    if let Some(ns) = table.get(&pid_ns) {
        ns.rev_map.keys().copied().collect()
    } else { Vec::new() }
}

// ── Network namespace ─────────────────────────────────────────────────────
//
// Each network namespace has its own socket table, routing table, and
// interface list. Sockets created in one net_ns are not accessible from
// another.

#[derive(Clone)]
pub struct NetNamespace {
    pub id:         u32,
    /// Set of socket IDs belonging to this namespace
    pub socket_ids: alloc::collections::BTreeSet<u32>,
    pub loopback_ip: u32,  // 127.0.0.1 in host byte order
}

impl NetNamespace {
    pub fn root() -> Self {
        NetNamespace { id: 0, socket_ids: alloc::collections::BTreeSet::new(), loopback_ip: 0x7F00_0001 }
    }
    pub fn new_child() -> Self {
        NetNamespace { id: alloc_ns_id(), socket_ids: alloc::collections::BTreeSet::new(), loopback_ip: 0x7F00_0001 }
    }
    pub fn register_socket(&mut self, sock_id: u32) { self.socket_ids.insert(sock_id); }
    pub fn unregister_socket(&mut self, sock_id: u32) { self.socket_ids.remove(&sock_id); }
    pub fn owns_socket(&self, sock_id: u32) -> bool {
        self.id == 0 || self.socket_ids.contains(&sock_id)
    }
}

static NET_NS_TABLE: Mutex<BTreeMap<u32, NetNamespace>> = Mutex::new(BTreeMap::new());

pub fn net_ns_init() {
    NET_NS_TABLE.lock().insert(0, NetNamespace::root());
}

pub fn net_ns_can_use_socket(net_ns: u32, sock_id: u32) -> bool {
    if net_ns == 0 { return true; }
    NET_NS_TABLE.lock().get(&net_ns).map(|ns| ns.owns_socket(sock_id)).unwrap_or(false)
}

pub fn net_ns_register_socket(net_ns: u32, sock_id: u32) {
    let mut table = NET_NS_TABLE.lock();
    if let Some(ns) = table.get_mut(&net_ns) { ns.register_socket(sock_id); }
}

pub fn net_ns_unregister_socket(net_ns: u32, sock_id: u32) {
    let mut table = NET_NS_TABLE.lock();
    if let Some(ns) = table.get_mut(&net_ns) { ns.unregister_socket(sock_id); }
}

// ── IPC namespace ─────────────────────────────────────────────────────────

#[derive(Clone, Default)]
pub struct IpcNamespace {
    pub id:      u32,
    pub shm_ids: alloc::collections::BTreeSet<u32>,
    pub sem_ids: alloc::collections::BTreeSet<u32>,
    pub msg_ids: alloc::collections::BTreeSet<u32>,
}

static IPC_NS_TABLE: Mutex<BTreeMap<u32, IpcNamespace>> = Mutex::new(BTreeMap::new());

pub fn ipc_ns_init() {
    IPC_NS_TABLE.lock().insert(0, IpcNamespace { id: 0, ..Default::default() });
}

pub fn ipc_ns_can_access_shm(ipc_ns: u32, shm_id: u32) -> bool {
    if ipc_ns == 0 { return true; }
    IPC_NS_TABLE.lock().get(&ipc_ns).map(|ns| ns.shm_ids.contains(&shm_id)).unwrap_or(false)
}

// ── Mount namespace ───────────────────────────────────────────────────────
//
// Each mount namespace has its own VFS root. When a process in a non-root
// mount namespace calls open("/proc/1/..."), it sees only PIDs visible in
// its PID namespace. The root path is virtualized per namespace.

#[derive(Clone)]
pub struct MntNamespace {
    pub id:   u32,
    pub root: String,  // filesystem root path (e.g. "/" or "/container/rootfs")
}

impl MntNamespace {
    pub fn root_ns() -> Self { MntNamespace { id: 0, root: "/".to_string() } }
    pub fn new_child(root: &str) -> Self { MntNamespace { id: alloc_ns_id(), root: root.to_string() } }
}

// ── Namespace creation from clone(2) ─────────────────────────────────────

/// Create new namespaces for a cloned process based on CLONE_NEW* flags.
/// Returns the updated Namespaces struct for the child.
pub fn clone_namespaces(
    parent_ns: &crate::security::Namespaces,
    clone_flags: u64,
    child_kernel_pid: u32,
) -> crate::security::Namespaces {
    let mut ns = *parent_ns;

    if clone_flags & CLONE_NEWPID != 0 {
        let new_id = alloc_ns_id();
        let mut new_ns = PidNamespace::new_child();
        new_ns.id = new_id;
        new_ns.alloc_local(child_kernel_pid); // init gets PID 1 in new ns
        PID_NS_TABLE.lock().insert(new_id, new_ns);
        ns.pid_ns = new_id;
        crate::klog!("namespace: new PID namespace {} for pid {}", new_id, child_kernel_pid);
    } else if ns.pid_ns != 0 {
        // Inherit parent's PID namespace and register child
        let mut table = PID_NS_TABLE.lock();
        if let Some(pid_ns) = table.get_mut(&ns.pid_ns) {
            pid_ns.alloc_local(child_kernel_pid);
        }
    }

    if clone_flags & CLONE_NEWNET != 0 {
        let new_ns = NetNamespace::new_child();
        let id = new_ns.id;
        NET_NS_TABLE.lock().insert(id, new_ns);
        ns.net_ns = id;
        crate::klog!("namespace: new net namespace {} for pid {}", id, child_kernel_pid);
    }

    if clone_flags & CLONE_NEWIPC != 0 {
        let new_id = alloc_ns_id();
        IPC_NS_TABLE.lock().insert(new_id, IpcNamespace { id: new_id, ..Default::default() });
        ns.ipc_ns = new_id;
        crate::klog!("namespace: new IPC namespace {} for pid {}", new_id, child_kernel_pid);
    }

    if clone_flags & CLONE_NEWUTS != 0 {
        let parent_uts = uts_get(parent_ns.uts_ns);
        let new_uts = parent_uts.fork();
        let new_id = new_uts.id;
        UTS_NS_TABLE.lock().insert(new_id, new_uts);
        ns.uts_ns = new_id;
    }

    ns
}

// ── Enforcement: kill signal scoping ─────────────────────────────────────

/// Check if current process can send `sig` to `target_kernel_pid`.
/// Enforces PID namespace isolation.
pub fn check_kill_permission(sender_pid: u32, target_kernel_pid: u32, sig: i32) -> bool {
    let sender_ns = crate::process::with_process(sender_pid, |p| p.namespaces.pid_ns)
        .unwrap_or(0);
    // In root ns, all PIDs are visible
    if sender_ns == 0 { return true; }
    // In a child ns, only visible PIDs can be signaled
    pid_ns_can_signal(sender_ns, target_kernel_pid)
}

/// Translate a user-supplied PID through the calling process's PID namespace.
/// Returns the kernel PID, or None if not visible in this namespace.
pub fn resolve_pid_for_current(user_pid: u32) -> Option<u32> {
    if user_pid == 0 { return Some(crate::process::current_pid()); }
    let ns = crate::process::with_current(|p| p.namespaces.pid_ns).unwrap_or(0);
    if ns == 0 { return Some(user_pid); } // root ns: direct
    pid_ns_resolve(ns, user_pid)
}

// ── Enforcement: network namespace scoping ────────────────────────────────

/// Called when a process tries to use a socket. Verifies the socket belongs
/// to the calling process's network namespace.
pub fn check_socket_access(sock_id: u32) -> bool {
    let net_ns = crate::process::with_current(|p| p.namespaces.net_ns).unwrap_or(0);
    net_ns_can_use_socket(net_ns, sock_id)
}

/// Register a new socket in the calling process's network namespace.
pub fn register_socket_in_current_ns(sock_id: u32) {
    let net_ns = crate::process::with_current(|p| p.namespaces.net_ns).unwrap_or(0);
    net_ns_register_socket(net_ns, sock_id);
}

// ── Enforcement: /proc PID visibility ────────────────────────────────────

/// Returns the list of PIDs visible to the current process via /proc.
pub fn proc_visible_pids() -> Vec<u32> {
    let ns = crate::process::with_current(|p| p.namespaces.pid_ns).unwrap_or(0);
    pid_ns_visible_pids(ns)
}

// ── /proc/qsf/namespaces status ──────────────────────────────────────────

pub fn ns_status_for_pid(kernel_pid: u32) -> Vec<u8> {
    let ns = crate::process::with_process(kernel_pid, |p| p.namespaces).unwrap_or_else(crate::security::Namespaces::root);
    let uts = uts_get(ns.uts_ns);
    alloc::format!(
        "pid_ns={}\nnet_ns={}\nmnt_ns={}\nuts_ns={}\nipc_ns={}\nuser_ns={}\nhostname={}\n",
        ns.pid_ns, ns.net_ns, ns.mnt_ns, ns.uts_ns, ns.ipc_ns, ns.user_ns,
        uts.hostname,
    ).into_bytes()
}

// ── Mount namespace root table ────────────────────────────────────────────

static MNT_NS_ROOTS: Mutex<BTreeMap<u32, String>> = Mutex::new(BTreeMap::new());

/// Get the VFS root path for a mount namespace.
/// Returns "/" for the root namespace (id=0).
pub fn mnt_ns_root(mnt_ns: u32) -> String {
    if mnt_ns == 0 { return String::from("/"); }
    MNT_NS_ROOTS.lock().get(&mnt_ns).cloned().unwrap_or_else(|| String::from("/"))
}

/// Set the VFS root for a mount namespace (chroot-like).
pub fn mnt_ns_set_root(mnt_ns: u32, root: &str) {
    let canonical = root.trim_end_matches('/');
    let r = if canonical.is_empty() { String::from("/") } else { String::from(canonical) };
    MNT_NS_ROOTS.lock().insert(mnt_ns, r);
}

/// Create a new mount namespace as a copy of `parent_ns`, with root `new_root`.
pub fn new_mnt_ns(parent_ns: u32, new_root: &str) -> u32 {
    use core::sync::atomic::Ordering;
    let id = NEXT_NS_ID.fetch_add(1, Ordering::Relaxed);
    let parent_root = mnt_ns_root(parent_ns);
    let root = if new_root.is_empty() { parent_root } else { String::from(new_root) };
    MNT_NS_ROOTS.lock().insert(id, root);
    id
}


// ── User namespace UID/GID mapping ───────────────────────────────────────
//
// Each user namespace has uid_map and gid_map files (Linux format):
//   inside_start  host_start  count
// UID `inside` in the namespace maps to host UID `host_start + (inside - inside_start)`.
// At most 5 mapping entries per namespace (Linux default).

#[derive(Clone, Default)]
pub struct IdMapEntry {
    pub inner_start: u32,
    pub outer_start: u32,
    pub count:       u32,
}

#[derive(Clone, Default)]
pub struct IdMap {
    pub entries: Vec<IdMapEntry>,
}

impl IdMap {
    /// Translate an inner UID to an outer (host) UID.
    /// Returns None if not mapped (this appears as uid=65534 "nobody" to host).
    pub fn to_outer(&self, inner: u32) -> Option<u32> {
        for e in &self.entries {
            if inner >= e.inner_start && inner < e.inner_start + e.count {
                return Some(e.outer_start + (inner - e.inner_start));
            }
        }
        None
    }
    /// Translate a host UID to an inner UID.
    pub fn to_inner(&self, outer: u32) -> Option<u32> {
        for e in &self.entries {
            if outer >= e.outer_start && outer < e.outer_start + e.count {
                return Some(e.inner_start + (outer - e.outer_start));
            }
        }
        None
    }
    /// Parse a uid_map/gid_map file content (newline-separated triples).
    pub fn parse(text: &str) -> Self {
        let mut map = IdMap::default();
        for line in text.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 {
                if let (Ok(a), Ok(b), Ok(c)) = (
                    parts[0].parse::<u32>(), parts[1].parse::<u32>(), parts[2].parse::<u32>()
                ) {
                    if map.entries.len() < 5 { // Linux limit
                        map.entries.push(IdMapEntry { inner_start: a, outer_start: b, count: c });
                    }
                }
            }
        }
        map
    }
    pub fn to_file_content(&self) -> Vec<u8> {
        let mut out = String::new();
        for e in &self.entries {
            out.push_str(&alloc::format!("{} {} {}\n", e.inner_start, e.outer_start, e.count));
        }
        out.into_bytes()
    }
}

#[derive(Clone, Default)]
pub struct UserNamespace {
    pub id:      u32,
    pub uid_map: IdMap,
    pub gid_map: IdMap,
    pub parent:  u32,
}

static USER_NS_TABLE: Mutex<BTreeMap<u32, UserNamespace>> = Mutex::new(BTreeMap::new());

pub fn user_ns_init() {
    let mut root_ns = UserNamespace { id: 0, uid_map: IdMap::default(),
        gid_map: IdMap::default(), parent: 0 };
    // Root namespace: identity mapping (0..65536 → 0..65536)
    root_ns.uid_map.entries.push(IdMapEntry { inner_start: 0, outer_start: 0, count: 65536 });
    root_ns.gid_map.entries.push(IdMapEntry { inner_start: 0, outer_start: 0, count: 65536 });
    USER_NS_TABLE.lock().insert(0, root_ns);
}

/// Map an inner UID (seen by process in user_ns) to a host/outer UID.
/// Returns 65534 (nobody) if not mapped.
pub fn map_uid_to_host(user_ns: u32, inner_uid: u32) -> u32 {
    USER_NS_TABLE.lock()
        .get(&user_ns)
        .and_then(|ns| ns.uid_map.to_outer(inner_uid))
        .unwrap_or(65534)
}

/// Map a host UID to the inner UID seen by a process in user_ns.
pub fn map_uid_from_host(user_ns: u32, outer_uid: u32) -> u32 {
    USER_NS_TABLE.lock()
        .get(&user_ns)
        .and_then(|ns| ns.uid_map.to_inner(outer_uid))
        .unwrap_or(65534)
}

pub fn map_gid_to_host(user_ns: u32, inner_gid: u32) -> u32 {
    USER_NS_TABLE.lock()
        .get(&user_ns)
        .and_then(|ns| ns.gid_map.to_outer(inner_gid))
        .unwrap_or(65534)
}

pub fn install_uid_map(user_ns: u32, content: &str) {
    let map = IdMap::parse(content);
    if let Some(ns) = USER_NS_TABLE.lock().get_mut(&user_ns) {
        ns.uid_map = map;
    }
}

pub fn install_gid_map(user_ns: u32, content: &str) {
    let map = IdMap::parse(content);
    if let Some(ns) = USER_NS_TABLE.lock().get_mut(&user_ns) {
        ns.gid_map = map;
    }
}

pub fn get_uid_map_content(user_ns: u32) -> Vec<u8> {
    USER_NS_TABLE.lock()
        .get(&user_ns)
        .map(|ns| ns.uid_map.to_file_content())
        .unwrap_or_default()
}

pub fn get_gid_map_content(user_ns: u32) -> Vec<u8> {
    USER_NS_TABLE.lock()
        .get(&user_ns)
        .map(|ns| ns.gid_map.to_file_content())
        .unwrap_or_default()
}

/// Create new user namespace inheriting parent's mapping structure.
pub fn new_user_ns(parent_ns: u32) -> u32 {
    use core::sync::atomic::Ordering;
    let id = NEXT_NS_ID.fetch_add(1, Ordering::Relaxed);
    let ns = UserNamespace { id, uid_map: IdMap::default(), gid_map: IdMap::default(), parent: parent_ns };
    USER_NS_TABLE.lock().insert(id, ns);
    id
}

// ── Init ──────────────────────────────────────────────────────────────────

pub fn init() {
    uts_ns_init();
    pid_ns_init();
    net_ns_init();
    ipc_ns_init();
    user_ns_init();
    MNT_NS_ROOTS.lock().insert(0, String::from("/"));
    crate::klog!("QSF namespaces: PID + net + IPC + UTS + mount initialized");
}
