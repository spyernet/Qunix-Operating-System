/*
* Copyright (c) 2026 Mohammad Muzamil
*
* This file is part of QunixOS, an operating system written in Rust.
* It is licensed under the Apache License, Version 2.0.
*/

use alloc::vec;
use alloc::vec::Vec;
// procfs — /proc filesystem providing per-process and system info.
// Dynamically generates content from kernel state at read time.

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use alloc::format;
use crate::vfs::*;

// ── Helpers ───────────────────────────────────────────────────────────────

fn proc_sb() -> Arc<Superblock> {
    Arc::new(Superblock { dev: 4, fs_type: String::from("proc"), ops: Arc::new(ProcFs) })
}

fn make_dir(ino: u64, ops: Arc<dyn InodeOps>) -> Inode {
    Inode { ino, mode: S_IFDIR|0o555, uid:0,gid:0,size:0,
            atime:0,mtime:0,ctime:0, ops, sb: proc_sb() }
}

fn make_ro_file(ino: u64, data: Vec<u8>) -> Inode {
    let size = data.len() as u64;
    Inode { ino, mode: S_IFREG|0o444, uid:0,gid:0,size,
            atime:0,mtime:0,ctime:0, ops: Arc::new(ProcData(data)), sb: proc_sb() }
}

fn make_rw_file(ino: u64, data: Vec<u8>) -> Inode {
    let size = data.len() as u64;
    Inode { ino, mode: S_IFREG|0o644, uid:0,gid:0,size,
            atime:0,mtime:0,ctime:0, ops: Arc::new(ProcData(data)), sb: proc_sb() }
}

/// Writable inode for /proc/qsf/policy — accepts MAC policy rules.
struct ProcQsfPolicy;
impl InodeOps for ProcQsfPolicy {
    fn read(&self, _: &Inode, buf: &mut [u8], _: u64) -> Result<usize, VfsError> {
        let data = b"# Write MAC policy rules to this file\n                      # Format: allow subject=0x... object=0x... access=read,write\n                      # mac enable / mac disable / mac default deny\n";
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Ok(n)
    }
    fn write(&self, _: &Inode, data: &[u8], _: u64) -> Result<usize, VfsError> {
        crate::security::mac_policy::handle_policy_write(data)
    }
    fn truncate(&self, _: &Inode, _: u64) -> Result<(), VfsError> { Ok(()) }
    // fn stat removed - not in InodeOps trait
    fn lookup(&self, _: &Inode, _: &str) -> Result<Inode, VfsError> { Err(ENOENT) }
    fn readdir(&self, _: &Inode, _offset: u64) -> Result<Vec<DirEntry>, VfsError> { Err(ENOTDIR) }
    fn create(&self,_:&Inode,_:&str,_:u32)->Result<Inode,VfsError>{Err(EACCES)}
    fn chmod(&self,_:&Inode,_:u32)->Result<(),VfsError>{Ok(())}
    fn chown(&self,_:&Inode,_:u32,_:u32)->Result<(),VfsError>{Ok(())}
}


fn make_lnk(ino: u64, target: &str) -> Inode {
    let target = target.to_string();
    Inode { ino, mode: S_IFLNK|0o777, uid:0,gid:0,size:target.len() as u64,
            atime:0,mtime:0,ctime:0,
            ops: Arc::new(ProcLink(target)), sb: proc_sb() }
}

struct ProcData(Vec<u8>);
impl InodeOps for ProcData {
    fn read(&self,_:&Inode,buf:&mut[u8],off:u64)->Result<usize,VfsError>{
        let s=off as usize; if s>=self.0.len(){return Ok(0);}
        let n=buf.len().min(self.0.len()-s); buf[..n].copy_from_slice(&self.0[s..s+n]); Ok(n)
    }
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Ok(0)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{Err(ENOTDIR)}
    fn lookup(&self,_:&Inode,_:&str)->Result<Inode,VfsError>{Err(ENOENT)}
}

struct ProcLink(String);
impl InodeOps for ProcLink {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Ok(0)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EACCES)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{Err(ENOTDIR)}
    fn lookup(&self,_:&Inode,_:&str)->Result<Inode,VfsError>{Err(ENOENT)}
    fn readlink(&self,_:&Inode)->Result<String,VfsError>{Ok(self.0.clone())}
}

// ── Root ops ──────────────────────────────────────────────────────────────

pub struct ProcFs;
impl ProcFs { pub fn new() -> Superblock { Superblock { dev:4, fs_type:String::from("proc"), ops:Arc::new(ProcFs) } } }
impl SuperblockOps for ProcFs { fn get_root(&self)->Result<Inode,VfsError>{ Ok(make_dir(1, Arc::new(ProcRoot))) } }

struct ProcRoot;
impl InodeOps for ProcRoot {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{
        let mut v = vec![
            DirEntry{name:".".into(),ino:1,file_type:4},
            DirEntry{name:"..".into(),ino:1,file_type:4},
            DirEntry{name:"version".into(),ino:10,file_type:8},
            DirEntry{name:"meminfo".into(),ino:11,file_type:8},
            DirEntry{name:"uptime".into(),ino:12,file_type:8},
            DirEntry{name:"cpuinfo".into(),ino:13,file_type:8},
            DirEntry{name:"cmdline".into(),ino:14,file_type:8},
            DirEntry{name:"mounts".into(),ino:15,file_type:8},
            DirEntry{name:"filesystems".into(),ino:16,file_type:8},
            DirEntry{name:"interrupts".into(),ino:17,file_type:8},
            DirEntry{name:"stat".into(),ino:18,file_type:8},
            DirEntry{name:"loadavg".into(),ino:19,file_type:8},
            DirEntry{name:"net".into(),ino:30,file_type:4},
            DirEntry{name:"sys".into(),ino:40,file_type:4},
            DirEntry{name:"self".into(),ino:50,file_type:4},
        ];
        for pid in crate::process::all_pids() {
            v.push(DirEntry{name:pid.to_string(),ino:1000+pid as u64,file_type:4});
        }
        Ok(v)
    }
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        match name {
            "version"     => Ok(make_ro_file(10, gen_version())),
            "meminfo"     => Ok(make_ro_file(11, gen_meminfo())),
            "uptime"      => Ok(make_ro_file(12, gen_uptime())),
            "cpuinfo"     => Ok(make_ro_file(13, gen_cpuinfo())),
            "cmdline"     => Ok(make_ro_file(14, b"BOOT_IMAGE=/kernel root=/dev/sda1\n".to_vec())),
            "mounts"      => Ok(make_ro_file(15, gen_mounts())),
            "filesystems" => Ok(make_ro_file(16, gen_filesystems())),
            "interrupts"  => Ok(make_ro_file(17, gen_interrupts())),
            "stat"        => Ok(make_ro_file(18, gen_stat())),
            "loadavg"     => Ok(make_ro_file(19, gen_loadavg())),
            "net"         => Ok(make_dir(30, Arc::new(ProcNet))),
            "sys"         => Ok(make_dir(40, Arc::new(ProcSys))),
            "self"        => {
                let pid = crate::process::current_pid();
                Ok(make_dir(50, Arc::new(ProcPidDir{pid})))
            }
            s if s.chars().all(|c| c.is_ascii_digit()) => {
                let pid: u32 = s.parse().map_err(|_| ENOENT)?;
                if crate::process::with_process(pid, |_| ()).is_none() { return Err(ENOENT); }
                Ok(make_dir(1000+pid as u64, Arc::new(ProcPidDir{pid})))
            }
            _ => Err(ENOENT),
        }
    }
}

// ── Per-pid directory ─────────────────────────────────────────────────────

struct ProcPidDir { pid: u32 }
impl InodeOps for ProcPidDir {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{
        let base = 1000 + self.pid as u64 * 100;
        Ok(vec![
            DirEntry{name:".".into(),ino:base,file_type:4},
            DirEntry{name:"..".into(),ino:1,file_type:4},
            DirEntry{name:"status".into(),ino:base+1,file_type:8},
            DirEntry{name:"stat".into(),ino:base+2,file_type:8},
            DirEntry{name:"cmdline".into(),ino:base+3,file_type:8},
            DirEntry{name:"maps".into(),ino:base+4,file_type:8},
            DirEntry{name:"mem".into(),ino:base+5,file_type:8},
            DirEntry{name:"fd".into(),ino:base+10,file_type:4},
            DirEntry{name:"exe".into(),ino:base+11,file_type:10},
            DirEntry{name:"cwd".into(),ino:base+12,file_type:10},
            DirEntry{name:"root".into(),ino:base+13,file_type:10},
            DirEntry{name:"environ".into(),ino:base+14,file_type:8},
            DirEntry{name:"smaps".into(),ino:base+15,file_type:8},
            DirEntry{name:"wchan".into(),ino:base+16,file_type:8},
            DirEntry{name:"schedstat".into(),ino:base+17,file_type:8},
            DirEntry{name:"oom_score".into(),ino:base+18,file_type:8},
            DirEntry{name:"oom_score_adj".into(),ino:base+19,file_type:8},
            DirEntry{name:"loginuid".into(),ino:base+20,file_type:8},
            DirEntry{name:"sessionid".into(),ino:base+21,file_type:8},
            DirEntry{name:"limits".into(),ino:base+22,file_type:8},
            DirEntry{name:"io".into(),ino:base+23,file_type:8},
        ])
    }
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        let pid = self.pid;
        let base = 1000 + pid as u64 * 100;
        match name {
            "status"       => Ok(make_ro_file(base+1, gen_pid_status(pid))),
            "stat"         => Ok(make_ro_file(base+2, gen_pid_stat(pid))),
            "cmdline"      => Ok(make_ro_file(base+3, gen_pid_cmdline(pid))),
            "maps"         => Ok(make_ro_file(base+4, gen_pid_maps(pid))),
            "mem"          => Ok(make_ro_file(base+5, alloc::vec![])),
            "fd"           => Ok(make_dir(base+10, Arc::new(ProcPidFdDir{pid}))),
            "exe"          => {
                let name = crate::process::with_process(pid, |p| p.name.clone()).unwrap_or_default();
                Ok(make_lnk(base+11, &name))
            }
            "cwd"          => {
                let cwd = crate::process::with_process(pid, |p| p.cwd.clone()).unwrap_or_else(|| String::from("/"));
                Ok(make_lnk(base+12, &cwd))
            }
            "root"         => Ok(make_lnk(base+13, "/")),
            "environ"      => Ok(make_ro_file(base+14, gen_environ())),
            "smaps"        => Ok(make_ro_file(base+15, gen_pid_smaps(pid))),
            "wchan"        => Ok(make_ro_file(base+16, b"0\n".to_vec())),
            "schedstat"    => Ok(make_ro_file(base+17, b"0 0 0\n".to_vec())),
            "oom_score"    => Ok(make_ro_file(base+18, b"0\n".to_vec())),
            "oom_score_adj"=> Ok(make_rw_file(base+19, b"0\n".to_vec())),
            "loginuid"     => Ok(make_ro_file(base+20, format!("{}\n", crate::process::with_process(pid,|p|p.uid).unwrap_or(0)).into_bytes())),
            "uid_map"      => Ok(make_rw_file(base+21, {
                let ns = crate::process::with_process(pid, |p| p.namespaces.user_ns).unwrap_or(0);
                crate::security::namespace::get_uid_map_content(ns)
            })),
            "gid_map"      => Ok(make_rw_file(base+22, {
                let ns = crate::process::with_process(pid, |p| p.namespaces.user_ns).unwrap_or(0);
                crate::security::namespace::get_gid_map_content(ns)
            })),
            "ns/user" | "ns/pid" | "ns/mnt" | "ns/net" | "ns/ipc" | "ns/uts" => {
                let nsname = name.strip_prefix("ns/").unwrap_or(name);
                let nsid = crate::process::with_process(pid, |p| match nsname {
                    "user" => p.namespaces.user_ns, "pid" => p.namespaces.pid_ns,
                    "mnt"  => p.namespaces.mnt_ns,  "net" => p.namespaces.net_ns,
                    "ipc"  => p.namespaces.ipc_ns,  "uts" => p.namespaces.uts_ns,
                    _      => 0,
                }).unwrap_or(0);
                Ok(make_lnk(base+23, &alloc::format!("{}:[{}]", nsname, nsid)))
            }
            "sessionid"    => Ok(make_ro_file(base+21, format!("{}\n", crate::process::with_process(pid,|p|p.sid).unwrap_or(0)).into_bytes())),
            "limits"       => Ok(make_ro_file(base+22, gen_limits())),
            "io"           => Ok(make_ro_file(base+23, b"rchar: 0\nwchar: 0\nsyscr: 0\nsyscw: 0\nread_bytes: 0\nwrite_bytes: 0\n".to_vec())),
            _ => Err(ENOENT),
        }
    }
}

// ── /proc/<pid>/fd directory ─────────────────────────────────────────────

struct ProcPidFdDir { pid: u32 }
impl InodeOps for ProcPidFdDir {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{
        let fds: Vec<u32> = crate::process::with_process(self.pid, |p| p.fds.keys().copied().collect()).unwrap_or_default();
        let mut v = vec![
            DirEntry{name:".".into(),ino:0,file_type:4},
            DirEntry{name:"..".into(),ino:0,file_type:4},
        ];
        for fd in fds {
            v.push(DirEntry{name:fd.to_string(),ino:fd as u64,file_type:10});
        }
        Ok(v)
    }
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        let fd: u32 = name.parse().map_err(|_| ENOENT)?;
        let target = crate::process::with_process(self.pid, |p|
            p.get_fd(fd).map(|f| f.inode.ino.to_string())
        ).flatten().unwrap_or_else(|| String::from("?"));
        Ok(make_lnk(fd as u64, &format!("/proc/{}/fd/{}", self.pid, fd)))
    }
}

// ── /proc/net ─────────────────────────────────────────────────────────────

struct ProcNet;
impl InodeOps for ProcNet {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{
        Ok(vec![
            DirEntry{name:"dev".into(),ino:31,file_type:8},
            DirEntry{name:"if_inet6".into(),ino:32,file_type:8},
            DirEntry{name:"tcp".into(),ino:33,file_type:8},
            DirEntry{name:"tcp6".into(),ino:34,file_type:8},
            DirEntry{name:"udp".into(),ino:35,file_type:8},
            DirEntry{name:"udp6".into(),ino:36,file_type:8},
            DirEntry{name:"unix".into(),ino:37,file_type:8},
            DirEntry{name:"fib_trie".into(),ino:38,file_type:8},
            DirEntry{name:"route".into(),ino:39,file_type:8},
            DirEntry{name:"arp".into(),ino:40,file_type:8},
            DirEntry{name:"sockstat".into(),ino:41,file_type:8},
            DirEntry{name:"protocols".into(),ino:42,file_type:8},
            DirEntry{name:"snmp".into(),ino:43,file_type:8},
        ])
    }
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        let data: Vec<u8> = match name {
            "dev"       => gen_net_dev(),
            "tcp"       => gen_net_tcp(),
            "udp"       => gen_net_udp(),
            "tcp6"      => b"  sl  local_address rem_address   st tx_queue rx_queue\n".to_vec(),
            "udp6"      => b"  sl  local_address rem_address   st\n".to_vec(),
            "unix"      => b"Num RefCount Protocol Flags    Type St Inode Path\n".to_vec(),
            "if_inet6"  => alloc::vec![],
            "arp"       => b"IP address       HW type     Flags       HW address            Mask     Device\n".to_vec(),
            "route"     => gen_net_route(),
            "fib_trie"  => alloc::vec![],
            "sockstat"  => b"sockets: used 0\nTCP: inuse 0 orphan 0 tw 0 alloc 0 mem 0\nUDP: inuse 0 mem 0\n".to_vec(),
            "protocols" => gen_net_protocols(),
            "snmp"      => b"Ip: Forwarding DefaultTTL\nIp: 1 64\n".to_vec(),
            _ => return Err(ENOENT),
        };
        Ok(make_ro_file(0, data))
    }
}

// ── /proc/sys ─────────────────────────────────────────────────────────────

struct ProcSys;
impl InodeOps for ProcSys {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{
        Ok(vec![
            DirEntry{name:"kernel".into(),ino:41,file_type:4},
            DirEntry{name:"vm".into(),ino:42,file_type:4},
            DirEntry{name:"net".into(),ino:43,file_type:4},
            DirEntry{name:"fs".into(),ino:44,file_type:4},
        ])
    }
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        match name {
            "kernel" => Ok(make_dir(41, Arc::new(ProcSysKernel))),
            "vm"     => Ok(make_dir(42, Arc::new(ProcSysVm))),
            "net"    => Ok(make_dir(43, Arc::new(ProcSysNet))),
            "fs"     => Ok(make_dir(44, Arc::new(ProcSysFs))),
            _ => Err(ENOENT),
        }
    }
}

struct ProcSysKernel;
impl InodeOps for ProcSysKernel {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{
        Ok(vec![
            DirEntry{name:"hostname".into(),ino:100,file_type:8},
            DirEntry{name:"ostype".into(),ino:101,file_type:8},
            DirEntry{name:"osrelease".into(),ino:102,file_type:8},
            DirEntry{name:"version".into(),ino:103,file_type:8},
            DirEntry{name:"pid_max".into(),ino:104,file_type:8},
            DirEntry{name:"threads-max".into(),ino:105,file_type:8},
            DirEntry{name:"randomize_va_space".into(),ino:106,file_type:8},
            DirEntry{name:"overcommit_memory".into(),ino:107,file_type:8},
            DirEntry{name:"printk".into(),ino:108,file_type:8},
            DirEntry{name:"dmesg_restrict".into(),ino:109,file_type:8},
            DirEntry{name:"ngroups_max".into(),ino:110,file_type:8},
            DirEntry{name:"shmmax".into(),ino:111,file_type:8},
            DirEntry{name:"shmall".into(),ino:112,file_type:8},
            DirEntry{name:"sem".into(),ino:113,file_type:8},
            DirEntry{name:"msgmax".into(),ino:114,file_type:8},
            DirEntry{name:"msgmnb".into(),ino:115,file_type:8},
            DirEntry{name:"msgmni".into(),ino:116,file_type:8},
        ])
    }
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        let data: Vec<u8> = match name {
            "hostname"           => b"qunix\n".to_vec(),
            "ostype"             => b"Linux\n".to_vec(),
            "osrelease"          => b"6.1.0-qunix\n".to_vec(),
            "version"            => b"#1 SMP PREEMPT_DYNAMIC Qunix 0.2.0\n".to_vec(),
            "pid_max"            => b"65536\n".to_vec(),
            "threads-max"        => b"65536\n".to_vec(),
            "randomize_va_space" => b"2\n".to_vec(),
            "overcommit_memory"  => b"0\n".to_vec(),
            "printk"             => b"4\t4\t1\t7\n".to_vec(),
            "dmesg_restrict"     => b"0\n".to_vec(),
            "ngroups_max"        => b"65536\n".to_vec(),
            "shmmax"             => b"18446744073692774399\n".to_vec(),
            "shmall"             => b"18446744073692774399\n".to_vec(),
            "sem"                => b"32000\t1024000000\t500\t32000\n".to_vec(),
            "msgmax"             => b"8192\n".to_vec(),
            "msgmnb"             => b"16384\n".to_vec(),
            "msgmni"             => b"32000\n".to_vec(),
            _ => return Err(ENOENT),
        };
        Ok(make_rw_file(0, data))
    }
}

struct ProcSysVm;
impl InodeOps for ProcSysVm {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{Ok(alloc::vec![])}
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        let data = match name {
            "overcommit_memory"  => b"0\n".to_vec(),
            "overcommit_ratio"   => b"50\n".to_vec(),
            "swappiness"         => b"60\n".to_vec(),
            "dirty_ratio"        => b"20\n".to_vec(),
            "mmap_min_addr"      => b"65536\n".to_vec(),
            "max_map_count"      => b"65530\n".to_vec(),
            "vfs_cache_pressure" => b"100\n".to_vec(),
            _ => return Err(ENOENT),
        };
        Ok(make_rw_file(0, data))
    }
}

struct ProcSysNet;
impl InodeOps for ProcSysNet {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{Ok(alloc::vec![])}
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        let data = match name {
            "ipv4/ip_forward"          => b"0\n".to_vec(),
            "ipv4/tcp_syncookies"      => b"1\n".to_vec(),
            "ipv4/tcp_max_syn_backlog" => b"4096\n".to_vec(),
            "core/somaxconn"           => b"128\n".to_vec(),
            _ => return Err(ENOENT),
        };
        Ok(make_rw_file(0, data))
    }
}

struct ProcSysFs;
impl InodeOps for ProcSysFs {
    fn read(&self,_:&Inode,_:&mut[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn write(&self,_:&Inode,_:&[u8],_:u64)->Result<usize,VfsError>{Err(EISDIR)}
    fn readdir(&self,_:&Inode,_:u64)->Result<Vec<DirEntry>,VfsError>{Ok(alloc::vec![])}
    fn lookup(&self,_:&Inode,name:&str)->Result<Inode,VfsError>{
        let data = match name {
            "file-max"   => b"9223372036854775807\n".to_vec(),
            "file-nr"    => b"3\t0\t9223372036854775807\n".to_vec(),
            "inode-max"  => b"9223372036854775807\n".to_vec(),
            "pipe-max-size" => b"1048576\n".to_vec(),
            _ => return Err(ENOENT),
        };
        Ok(make_rw_file(0, data))
    }
}

// ── Content generators ────────────────────────────────────────────────────

fn gen_version() -> Vec<u8> {
    format!("Linux version 6.1.0-qunix (qunix@localhost) (rustc nightly) #1 SMP PREEMPT_DYNAMIC {}\n",
        crate::time::realtime_secs()).into_bytes()
}

fn gen_meminfo() -> Vec<u8> {
    let total_kb = crate::memory::phys::total_frames() * 4;
    let free_kb  = crate::memory::phys::free_frames() * 4;
    let used_kb  = total_kb - free_kb;
    let heap_used = crate::memory::heap::used() / 1024;
    format!(
"MemTotal:       {:>8} kB\nMemFree:        {:>8} kB\nMemAvailable:   {:>8} kB\n\
Buffers:               0 kB\nCached:                0 kB\nSwapCached:            0 kB\n\
Active:         {:>8} kB\nInactive:              0 kB\nMapped:                0 kB\n\
Shmem:                 0 kB\nKernelStack:    {:>8} kB\nPageTables:            0 kB\n\
Slab:                  0 kB\nSReclaimable:          0 kB\nSUnreclaim:            0 kB\n\
SwapTotal:             0 kB\nSwapFree:              0 kB\nDirty:                 0 kB\n\
Writeback:             0 kB\nAnonPages:             0 kB\nCommitLimit:    {:>8} kB\n\
Committed_AS:          0 kB\nVmallocTotal:   {:>8} kB\nVmallocUsed:           0 kB\n\
HugePagesTotal:        0\nHugePagesFree:         0\nHugepagesize:       2048 kB\n",
        total_kb, free_kb, free_kb, used_kb, heap_used, total_kb, 256 * 1024 * 1024
    ).into_bytes()
}

fn gen_uptime() -> Vec<u8> {
    let up_s  = crate::time::uptime_ms() as f64 / 1000.0;
    let idle  = up_s * 0.9; // fake idle
    format!("{:.2} {:.2}\n", up_s, idle).into_bytes()
}

fn gen_cpuinfo() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("processor\t: 0\n");
    s.push_str("vendor_id\t: QunixX86\n");
    s.push_str("cpu family\t: 6\n");
    s.push_str("model\t\t: 60\n");
    s.push_str("model name\t: Qunix Virtual Processor @ 2.000GHz\n");
    s.push_str("stepping\t: 3\n");
    s.push_str("cpu MHz\t\t: 2000.000\n");
    s.push_str("cache size\t: 6144 KB\n");
    s.push_str("physical id\t: 0\n");
    s.push_str("siblings\t: 1\n");
    s.push_str("core id\t\t: 0\n");
    s.push_str("cpu cores\t: 1\n");
    s.push_str("flags\t\t: fpu vme de pse tsc msr pae mce cx8 apic sep mtrr pge mca cmov pat pse36 clflush mmx fxsr sse sse2 syscall nx rdtscp lm constant_tsc\n");
    s.push_str("bogomips\t: 4000.00\n");
    s.push_str("clflush size\t: 64\n");
    s.push_str("cache_alignment\t: 64\n");
    s.push_str("address sizes\t: 40 bits physical, 48 bits virtual\n\n");
    s.into_bytes()
}

fn gen_mounts() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("tmpfs / tmpfs rw,relatime 0 0\n");
    s.push_str("devfs /dev devfs rw,relatime 0 0\n");
    s.push_str("proc /proc proc rw,nosuid,nodev,noexec,relatime 0 0\n");
    s.push_str("sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0\n");
    s.into_bytes()
}

fn gen_filesystems() -> Vec<u8> {
    b"nodev\tsysfs\nnodev\tproc\nnodev\tdevfs\nnodev\ttmpfs\n\text4\n\tfat\n\tvfat\n".to_vec()
}

fn gen_interrupts() -> Vec<u8> {
    let mut s = String::new();
    s.push_str("           CPU0\n");
    s.push_str("  0:          0   IO-APIC  2-edge  timer\n");
    s.push_str("  1:          0   IO-APIC  1-edge  i8042\n");
    s.push_str("NMI:          0   Non-maskable interrupts\n");
    s.push_str("ERR:          0\n");
    s.push_str("MIS:          0\n");
    s.into_bytes()
}

fn gen_stat() -> Vec<u8> {
    let t  = crate::time::ticks();
    let hz = 100u64;
    let up = t * hz / 1000;
    let pids = crate::process::all_pids().len();
    format!(
"cpu  {} 0 0 {} 0 0 0 0 0 0\ncpu0 {} 0 0 {} 0 0 0 0 0 0\n\
intr 0\nctxt 0\nbtime {}\nprocesses {}\nprocs_running 1\nprocs_blocked 0\n\
softirq 0 0 0 0 0 0 0 0 0 0 0\n",
        up, up * 10, up, up * 10,
        crate::time::realtime_secs(), pids
    ).into_bytes()
}

fn gen_loadavg() -> Vec<u8> {
    let nr = crate::sched::nr_running();
    let total = crate::process::all_pids().len();
    let pid = crate::process::current_pid();
    format!("0.00 0.00 0.00 {}/{} {}\n", nr, total, pid).into_bytes()
}

fn gen_pid_status(pid: u32) -> Vec<u8> {
    let (name, state, uid, gid, ppid, vmrss, threads) =
        crate::process::with_process(pid, |p| (
            p.name.clone(),
            match p.state {
                crate::process::ProcessState::Running  => "R (running)",
                crate::process::ProcessState::Runnable => "R (runnable)",
                crate::process::ProcessState::Sleeping => "S (sleeping)",
                crate::process::ProcessState::Stopped  => "T (stopped)",
                crate::process::ProcessState::Zombie(_)=> "Z (zombie)",
            },
            p.uid, p.gid, p.ppid,
            p.address_space.regions.len() * 4096,
            1usize,
        )).unwrap_or_else(|| (String::from("?"),"?",0,0,0,0,1));
    format!(
"Name:\t{}\nUmask:\t0022\nState:\t{}\nTgid:\t{}\nNgid:\t0\nPid:\t{}\nPPid:\t{}\n\
TracerPid:\t0\nUid:\t{} {} {} {}\nGid:\t{} {} {} {}\nFDSize:\t64\n\
VmPeak:\t{} kB\nVmSize:\t{} kB\nVmLck:\t0 kB\nVmPin:\t0 kB\n\
VmHWM:\t{} kB\nVmRSS:\t{} kB\nRssAnon:\t{} kB\n\
Threads:\t{}\nSigPnd:\t0000000000000000\nShdPnd:\t0000000000000000\n\
SigBlk:\t0000000000000000\nSigIgn:\t0000000000000000\nSigCgt:\t0000000000000000\n\
CapInh:\t0000000000000000\nCapPrm:\t0000000000003fff\nCapEff:\t0000000000003fff\n\
NoNewPrivs:\t0\nSeccomp:\t0\nSeccomp_filters:\t0\n",
        name, state, ppid, pid, ppid,
        uid, uid, uid, uid, gid, gid, gid, gid,
        vmrss / 1024, vmrss / 1024, vmrss / 1024, vmrss / 1024, vmrss / 1024,
        threads
    ).into_bytes()
}

fn gen_pid_stat(pid: u32) -> Vec<u8> {
    let (name, state_c, ppid, pgrp) =
        crate::process::with_process(pid, |p| (
            p.name.clone(),
            match p.state {
                crate::process::ProcessState::Running | crate::process::ProcessState::Runnable => 'R',
                crate::process::ProcessState::Sleeping => 'S',
                crate::process::ProcessState::Stopped  => 'T',
                crate::process::ProcessState::Zombie(_)=> 'Z',
            },
            p.ppid, p.pgid,
        )).unwrap_or_else(|| (String::from("?"), '?', 0, 0));
    format!("{} ({}) {} {} {} 0 0 0 0 0 0 0 0 0 0 20 0 1 0 {} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0\n",
        pid, name, state_c, ppid, pgrp, crate::time::ticks()
    ).into_bytes()
}

fn gen_pid_cmdline(pid: u32) -> Vec<u8> {
    crate::process::with_process(pid, |p| {
        let mut v = p.name.as_bytes().to_vec();
        v.push(0);
        v
    }).unwrap_or_default()
}

fn gen_pid_maps(pid: u32) -> Vec<u8> {
    let regions = crate::process::with_process(pid, |p| p.address_space.regions.clone()).unwrap_or_default();
    let mut s = String::new();
    for r in &regions {
        let prot_r = if r.prot.contains(crate::memory::vmm::Prot::READ)  { 'r' } else { '-' };
        let prot_w = if r.prot.contains(crate::memory::vmm::Prot::WRITE) { 'w' } else { '-' };
        let prot_x = if r.prot.contains(crate::memory::vmm::Prot::EXEC)  { 'x' } else { '-' };
        let kind_c = match &r.kind {
            crate::memory::vmm::RegionKind::Stack     => 'p',
            crate::memory::vmm::RegionKind::Anonymous => 'p',
            _                                          => 'p',
        };
        let name_str = match &r.kind {
            crate::memory::vmm::RegionKind::Stack     => String::from("[stack]"),
            crate::memory::vmm::RegionKind::Heap      => String::from("[heap]"),
            crate::memory::vmm::RegionKind::Vdso      => String::from("[vdso]"),
            _                                          => String::new(),
        };
        s.push_str(&format!("{:016x}-{:016x} {}{}{}{} 00000000 00:00 0",
            r.start, r.end, prot_r, prot_w, prot_x, kind_c));
        if !name_str.is_empty() { s.push_str(&format!("          {}", name_str)); }
        s.push('\n');
    }
    s.into_bytes()
}

fn gen_pid_smaps(pid: u32) -> Vec<u8> {
    let maps = gen_pid_maps(pid);
    // Augment each line with size info
    let mut out = String::new();
    for line in core::str::from_utf8(&maps).unwrap_or("").lines() {
        out.push_str(line);
        out.push('\n');
        out.push_str("Size:                  4 kB\nRss:                   4 kB\nPss:                   4 kB\n");
        out.push_str("Shared_Clean:          0 kB\nShared_Dirty:          0 kB\n");
        out.push_str("Private_Clean:         0 kB\nPrivate_Dirty:         4 kB\n");
        out.push_str("Referenced:            4 kB\nAnonymous:             4 kB\n");
        out.push_str("Swap:                  0 kB\nSwapPss:               0 kB\nLocked:                0 kB\n");
    }
    out.into_bytes()
}

fn gen_limits() -> Vec<u8> {
    b"Limit                     Soft Limit           Hard Limit           Units\n\
Max cpu time              unlimited            unlimited            seconds\n\
Max file size             unlimited            unlimited            bytes\n\
Max data size             unlimited            unlimited            bytes\n\
Max stack size            8388608              unlimited            bytes\n\
Max core file size        0                    unlimited            bytes\n\
Max resident set          unlimited            unlimited            bytes\n\
Max processes             65536                65536                processes\n\
Max open files            1024                 1024                 files\n\
Max locked memory         65536                65536                bytes\n\
Max address space         unlimited            unlimited            bytes\n\
Max file locks            unlimited            unlimited            locks\n\
Max pending signals       7804                 7804                 signals\n\
Max msgqueue size         819200               819200               bytes\n\
Max nice priority         0                    0\n\
Max realtime priority     0                    0\n\
Max realtime timeout      unlimited            unlimited            us\n".to_vec()
}

fn gen_environ() -> Vec<u8> {
    b"PATH=/bin:/sbin\0HOME=/\0TERM=xterm-256color\0LANG=en_US.UTF-8\0".to_vec()
}

fn gen_net_dev() -> Vec<u8> {
    b"Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
    lo:       0       0    0    0    0     0          0         0        0       0    0    0    0     0       0          0\n".to_vec()
}

fn gen_net_tcp() -> Vec<u8> {
    b"  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n".to_vec()
}

fn gen_net_udp() -> Vec<u8> {
    b"  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n".to_vec()
}

fn gen_net_route() -> Vec<u8> {
    b"Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
lo\t0000007F\t00000000\t0001\t0\t0\t0\tFF000000\t0\t0\t0\n".to_vec()
}

fn gen_net_protocols() -> Vec<u8> {
    b"protocol  size sockets  memory press maxhdr  slab module     cl co di ac io in de sh ss gs se re sp bi br ha uh gp em\n\
TCP       1984      0      -1 no      0   yes kernel      y  y  y  y  y  y  y  y  y  y  y  y  y  y  y  y  y  y\n\
UDP       1024      0      -1 no      0   yes kernel      y  y  y  n  n  n  n  n  n  n  n  n  n  n  n  n  n  n\n".to_vec()
}
