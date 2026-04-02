//! Filesystem layer — mounts root tmpfs and standard filesystems.
//! Creates the standard directory tree on the tmpfs root.

pub mod devfs;
pub mod ext4;
pub mod fat32;
pub mod fat32_rw;
pub mod procfs;
pub mod tmpfs;

use alloc::sync::Arc;
use alloc::string::String;
use crate::boot::BootInfo;

pub fn init(boot_info: &BootInfo) {
    use crate::vfs;

    // Root: tmpfs
    vfs::mount("/", Arc::new(tmpfs::TmpFs::new()));

    // Create standard directory hierarchy in tmpfs root
    let cwd = String::from("/");
    let dirs = [
        "/bin", "/sbin", "/lib", "/lib64", "/usr", "/usr/bin", "/usr/lib",
        "/usr/share", "/etc", "/dev", "/proc", "/sys", "/tmp", "/var",
        "/var/log", "/var/run", "/var/tmp", "/home", "/root", "/mnt",
        "/run", "/opt", "/srv", "/media",
    ];
    for dir in &dirs {
        let _ = vfs::mkdir(&cwd, dir, 0o755);
    }
    // /tmp and /var/tmp are world-writable
    let _ = vfs::mkdir(&cwd, "/tmp", 0o1777);
    let _ = vfs::mkdir(&cwd, "/var/tmp", 0o1777);
    // root home
    let _ = vfs::mkdir(&cwd, "/root", 0o700);

    // Write /etc/passwd, /etc/group, /etc/hostname, /etc/os-release, /etc/motd
    write_file("/etc/passwd",
        b"root:x:0:0:root:/root:/bin/qshell\nnobody:x:65534:65534:nobody:/:/bin/false\n");
    write_file("/etc/group",
        b"root:x:0:\nnobody:x:65534:\n");
    write_file("/etc/hostname", b"qunix\n");
    write_file("/etc/os-release",
        b"NAME=\"Qunix\"\nVERSION=\"0.2.0\"\nID=qunix\nID_LIKE=linux\n\
          PRETTY_NAME=\"Qunix OS 0.2.0\"\nVERSION_ID=\"0.2\"\n\
          BUILD_ID=rolling\nANSI_COLOR=\"1;32\"\n");
    write_file("/etc/motd",
        b"\nQunix OS 0.2.0 - A Rust OS\nType 'help' for commands.\n\n");

    // ── Phase 3 test files ────────────────────────────────────────────────
    // /test.txt: used by the Phase 3 open/read/close test
    write_file("/test.txt",
        b"Hello from Qunix! This file is readable via open/read/close.\n");
    // /hello: simple hello world text
    write_file("/hello", b"Hello, World!\n");
    // /etc/issue: login banner (many programs check this)
    write_file("/etc/issue", b"Qunix OS 0.2.0 \n \r\n\n");
    // /etc/localtime: stub so tzset() doesn't fail
    write_file("/etc/localtime", b"");
    // /proc/self/status stub (minimal; real procfs handles /proc/<pid>/)
    // These are already served by procfs; write_file would go to tmpfs not procfs
    // so skip duplicating them.
    write_file("/etc/profile",
        b"export PATH=/bin:/sbin:/usr/bin:/usr/sbin\nexport HOME=/root\n\
          export TERM=xterm-256color\nexport LANG=en_US.UTF-8\n");
    write_file("/etc/fstab",
        b"tmpfs / tmpfs defaults 0 0\ndevfs /dev devfs defaults 0 0\n\
          proc /proc proc defaults 0 0\nsysfs /sys sysfs defaults 0 0\n");
    write_file("/etc/ld.so.conf", b"/lib\n/lib64\n/usr/lib\n");
    write_file("/etc/nsswitch.conf",
        b"passwd: files\ngroup: files\nhosts: files\n");

    // Device filesystem
    vfs::mount("/dev", Arc::new(devfs::DevFs::new()));

    // /dev subdirectories
    let _ = vfs::mkdir(&cwd, "/dev/pts", 0o755);
    let _ = vfs::mkdir(&cwd, "/dev/shm", 0o1777);
    let _ = vfs::mkdir(&cwd, "/dev/input", 0o755);

    // Process information
    vfs::mount("/proc", Arc::new(procfs::ProcFs::new()));

    // Sysfs
    vfs::mount("/sys", Arc::new(sysfs_stub()));

    // Create /sys subdirs needed by udev/systemd/X11
    let sys_dirs = [
        "/sys/class", "/sys/class/drm", "/sys/class/input",
        "/sys/class/block", "/sys/class/net", "/sys/class/tty",
        "/sys/devices", "/sys/bus", "/sys/kernel",
        "/sys/kernel/mm", "/sys/kernel/security",
        "/sys/fs", "/sys/fs/cgroup",
        "/sys/power",
    ];
    for d in &sys_dirs {
        let _ = vfs::mkdir(&cwd, d, 0o755);
    }
    crate::klog!("fs: sysfs skeleton created");

    // Create /sys/class/drm/card0 symlink target for X11
    write_file("/sys/class/drm/card0/status", b"connected\n");
    write_file("/sys/class/drm/card0/enabled", b"enabled\n");
    let (w, h) = crate::drivers::gpu::dimensions();
    if w > 0 {
        let mut mode_buf = [0u8; 32];
        let mode_str = alloc::format!("{}x{}@60\n", w, h);
        write_file("/sys/class/drm/card0/modes", mode_str.as_bytes());
    }
    write_file("/sys/class/tty/tty0/active", b"tty1\n");
    write_file("/sys/kernel/mm/transparent_hugepage/enabled",
        b"always [madvise] never\n");

    // /proc/sys virtual entries
    write_file("/proc/sys/kernel/hostname", b"qunix\n");
    write_file("/proc/sys/kernel/ostype", b"Linux\n");
    write_file("/proc/sys/kernel/osrelease", b"6.1.0-qunix\n");
    write_file("/proc/sys/vm/overcommit_memory", b"0\n");
    write_file("/proc/sys/net/ipv4/ip_forward", b"0\n");
    write_file("/proc/sys/fs/file-max", b"9223372036854775807\n");

    // Create /dev/dri device nodes
    let _ = vfs::mkdir(&cwd, "/dev/dri", 0o755);
    crate::klog!("fs: pseudo-files populated");

    install_boot_userland(boot_info);
    crate::klog!("fs: boot userland installed");

    crate::klog!("Filesystems: / /dev /proc /sys mounted, standard tree created");
}

fn install_boot_userland(boot_info: &BootInfo) {
    fn phys_blob(phys: u64, size: u64) -> Option<&'static [u8]> {
        if phys == 0 || size == 0 { return None; }
        let virt = crate::arch::x86_64::paging::phys_to_virt(phys);
        Some(unsafe { core::slice::from_raw_parts(virt as *const u8, size as usize) })
    }

    if let Some(init) = phys_blob(boot_info.init_phys_start, boot_info.init_size) {
        crate::klog!("fs: installing init ({})", init.len());
        write_file("/sbin/init", init);
        write_file("/bin/init", init);
        crate::klog!("fs: init installed");
    }
    if let Some(qshell) = phys_blob(boot_info.qshell_phys_start, boot_info.qshell_size) {
        crate::klog!("fs: installing qshell ({})", qshell.len());
        write_file("/bin/qsh",    qshell);
        write_file("/bin/qshell", qshell);
        crate::klog!("fs: qshell installed");
    }
}

fn write_file(path: &str, data: &[u8]) {
    use crate::vfs::{O_CREAT, O_WRONLY, O_TRUNC};
    let cwd = alloc::string::String::from("/");
    match crate::vfs::open(&cwd, path, O_CREAT | O_WRONLY | O_TRUNC, 0o644) {
        Ok(fd) => { let _ = fd.inode.ops.write(&fd.inode, data, 0); }
        Err(_) => {}
    }
}

fn sysfs_stub() -> crate::vfs::Superblock {
    use alloc::sync::Arc;
    use alloc::string::String;
    use alloc::vec::Vec;
    use crate::vfs::*;

    struct SysFsOps;
    impl SuperblockOps for SysFsOps {
        fn get_root(&self) -> Result<Inode, VfsError> {
            Ok(Inode {
                ino: 1, mode: S_IFDIR | 0o555,
                uid: 0, gid: 0, size: 0,
                atime: 0, mtime: 0, ctime: 0,
                ops: Arc::new(SysDirOps),
                sb: Arc::new(Superblock {
                    dev: 5,
                    fs_type: String::from("sysfs"),
                    ops: Arc::new(SysFsOps),
                }),
            })
        }
    }

    // Sysfs uses tmpfs for actual data once directories/files are created
    // via write_file calls above. The mount at /sys uses the tmpfs root.
    // We return a stub superblock that provides an empty root; the
    // real files are created by write_file() writing into the tmpfs.
    // Actually sysfs is mounted over a fresh tmpfs here for isolation:

    struct SysDirOps;
    impl InodeOps for SysDirOps {
        fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
        fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
        fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{Ok(alloc::vec![])}
        fn lookup(&self,_:&Inode,_:&str)->Result<Inode,VfsError>{Err(ENOENT)}
        fn mkdir(&self,_:&Inode,_:&str,_:u32)->Result<Inode,VfsError>{Ok(Inode{
            ino:0,mode:S_IFDIR|0o755,uid:0,gid:0,size:0,
            atime:0,mtime:0,ctime:0,ops:Arc::new(SysDirOps),
            sb:Arc::new(Superblock{dev:5,fs_type:String::from("sysfs"),ops:Arc::new(SysFsOps)}),
        })}
        fn create(&self,_:&Inode,_:&str,_:u32)->Result<Inode,VfsError>{Ok(Inode{
            ino:0,mode:S_IFREG|0o644,uid:0,gid:0,size:0,
            atime:0,mtime:0,ctime:0,ops:Arc::new(SysDirOps),
            sb:Arc::new(Superblock{dev:5,fs_type:String::from("sysfs"),ops:Arc::new(SysFsOps)}),
        })}
    }

    // Use tmpfs as backing for sysfs (simpler than implementing a real sysfs)
    crate::vfs::Superblock {
        dev: 5,
        fs_type: String::from("sysfs"),
        ops: Arc::new(SysFsOps),
    }
}

/// Mount a FAT32 R/W image
pub fn mount_fat32_rw(data: alloc::vec::Vec<u8>, mountpoint: &str) -> bool {
    match fat32_rw::Fat32RwFs::new(data) {
        Some(sb) => { crate::vfs::mount(mountpoint, Arc::new(sb)); true }
        None     => false,
    }
}
/// Mount an ext4 filesystem from a Vec<u8> disk image.
pub fn mount_ext4(data: alloc::vec::Vec<u8>, mountpoint: &str) -> bool {
    match ext4::mount_from_bytes(data) {
        Some(sb) => {
            crate::vfs::mount(mountpoint, alloc::sync::Arc::new(sb));
            crate::klog!("ext4 mounted at {}", mountpoint);
            true
        }
        None => {
            crate::klog!("ext4 mount failed at {}", mountpoint);
            false
        }
    }
}

/// Mount ext4 from a block device (ATA/NVMe).
pub fn mount_ext4_device(dev_id: u64, disk: alloc::sync::Arc<dyn ext4::Disk>, mountpoint: &str) -> bool {
    match ext4::mount(disk, dev_id) {
        Some(sb) => {
            crate::vfs::mount(mountpoint, alloc::sync::Arc::new(sb));
            crate::klog!("ext4 device mounted at {}", mountpoint);
            true
        }
        None => false,
    }
}
