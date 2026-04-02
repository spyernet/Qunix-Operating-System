# Qunix Security

Qunix is a Unix-like operating system kernel written entirely in Rust, leveraging language-level memory safety features combined with traditional OS-level security mechanisms to provide a robust and secure computing environment.

## Table of Contents

- [Security Philosophy](#security-philosophy)
- [Memory Safety](#memory-safety)
- [Access Control](#access-control)
- [Kernel Security Features](#kernel-security-features)
- [Known Limitations](#known-limitations)
- [Security Best Practices](#security-best-practices)
- [Vulnerability Reporting](#vulnerability-reporting)
- [Security Roadmap](#security-roadmap)

## Security Philosophy

Qunix's security model is built on two complementary layers:

1. **Language-level Safety**: Rust's ownership system eliminates entire classes of bugs (buffer overflows, use-after-free, data races) at compile time
2. **Kernel-level Enforcement**: Traditional OS mechanisms (permissions, privilege separation, access controls) provide policy enforcement

This defense-in-depth approach reduces attack surface while maintaining OS-level flexibility and compatibility.

## Memory Safety

### Rust's Guarantees

All kernel code is written in Rust, providing:

- **Memory Safety**: No buffer overflows, use-after-free, or double-free vulnerabilities
- **Type Safety**: Strong typing prevents invalid state transitions
- **Thread Safety**: Ownership rules prevent data races at the type system level
- **Safe Abstractions**: Unsafe blocks are isolated and auditable

### Unsafe Code Management

While Rust prevents entire categories of bugs, some kernel functionality requires `unsafe` code (memory mapping, hardware access, FFI). Qunix minimizes unsafe code and isolates it in:

- **Architecture layer** (`arch/x86_64/`) — CPU-specific operations, page tables, GDT/IDT
- **Driver layer** (`drivers/`) — Hardware interaction
- **Memory layer** (`memory/`) — Physical frame allocation and VM operations

All `unsafe` blocks are documented with:
- Clear explanation of why `unsafe` is required
- Description of invariants that must be maintained
- Safety assertions where possible

### Unsafe Code Audit

To find all `unsafe` blocks in the kernel:

```bash
grep -r "unsafe" kernel/src/
```

## Access Control

### User and Group Permissions

Qunix implements POSIX-compliant permission model:

- **UID/GID System**: Each process has real and effective user/group IDs
- **File Permissions**: Standard rwx permissions for owner, group, others
- **Special Bits**: Setuid, setgid, and sticky bit support
- **Capability System**: Fine-grained privilege isolation (under development)

### Privilege Escalation

- **setuid binaries**: Enable temporary privilege elevation with security checks
- **Capability Dropping**: Processes can permanently drop privileges
- **UID/GID Switching**: `setuid()`, `seteuid()`, `setgid()` syscalls properly implemented
- **Root Identification**: UID 0 has special privileges; non-root cannot perform privileged operations

### Credential Management

```c
// Example: Checking permissions in kernel
fn check_permission(inode: &Inode, mode: u32, cred: &Credentials) -> Result<()> {
    if cred.uid == 0 {
        return Ok(()); // Root bypasses checks
    }
    
    if cred.uid == inode.uid {
        // Check owner permissions
    } else if cred.gid == inode.gid {
        // Check group permissions
    } else {
        // Check other permissions
    }
}
```

## Kernel Security Features

### Process Isolation

- **Address Space Layout Randomization (ASLR)**: Stack, heap, and code addresses randomized at process creation
- **Memory Protection**: Each process has isolated virtual address space
- **Resource Limits**: Per-process limits on memory, file descriptors, stack size
- **Namespace Support**: Process, IPC, and network namespace isolation (in development)

### Signal Delivery Security

- **Signal Validation**: Only the process owner or root can send signals
- **Signal Masking**: Processes can selectively block signals
- **Signal Handlers**: Executed in user space with proper context validation
- **Sigreturn Hardening**: Validated through signal frame integrity checks

### File System Security

- **Path Traversal Protection**: Symlink limits prevent infinite loops
- **Secure Temporary Files**: `mktemp()` uses cryptographically random names
- **Device Node Protection**: `/dev/` access controlled by device permissions
- **immutable/append-only Flags**: File attributes prevent unauthorized modification

### TTY and Console Security

- **TTY Access Control**: Terminal devices require proper permissions
- **Job Control**: Process groups prevent unauthorized terminal access
- **Session Management**: Credentials validated per session
- **Password Input**: TTY can operate in noecho mode for secure input

### IPC Security

- **Pipe Permissions**: Inherited from creator process's credentials
- **Shared Memory Keys**: Protected by System V IPC permissions
- **Message Queue ACL**: Per-queue access control
- **Futex Validation**: Futex syscalls validate address ownership

### Network Security (Planned)

- **Firewall Integration**: netfilter-style packet filtering
- **MAC Address Filtering**: Hardware-level ingress filtering
- **Default Deny Policy**: Whitelist-based approach to network access
- **Socket Credentials**: SOL_SOCKET SO_PEERCRED for IPC verification

## Known Limitations

### Current Implementation Status

The following security features are **not yet fully implemented**:

1. **SELinux/AppArmor**: Mandatory Access Control (MAC) is under development
2. **Full Namespace Support**: Partial implementation; user and PID namespaces incomplete
3. **Seccomp**: System call filtering not yet available
4. **Memory Tagging Extensions (MTE)**: Hardware-assisted memory protection not implemented
5. **Stack Canaries**: Limited stack overflow protection
6. **Full ASLR**: Partial implementation; some address ranges not randomized
7. **Kernel Hardening**: Some hardening mitigations (CFI, CET) not yet enabled

### Security Considerations for Users

1. **Development Status**: Qunix is under active development; security should be considered experimental
2. **Testing Recommended**: Run only trusted code and test thoroughly in isolated environments
3. **No Formal Verification**: While Rust provides safety, functionality has not undergone formal verification
4. **Userland Needs Hardening**: Security depends on secure userland utilities (under development)
5. **QEMU/Emulation Only**: Currently intended for emulation; hardware support is limited

### Privilege Escalation Vectors

- Unsecured setuid binaries may be vulnerable
- Kernel modules (not currently supported, but planned) could bypass protections
- Speculative execution attacks (side-channels) not mitigated
- Timing attacks possible in cryptographic operations

## Security Best Practices

### For OS Users

1. **Run Only Trusted Code**: Only execute binaries from trusted sources
2. **Use Appropriate Permissions**: Apply principle of least privilege to files and processes
3. **Monitor Processes**: Use `ps` and system monitoring to detect suspicious activity
4. **Isolate Critical Services**: Run sensitive services in separate processes with limited privileges
5. **Secure Communication**: Use encryption for network communication when available
6. **Regular Updates**: Stay current with kernel and userland updates

### For Kernel Development

1. **Code Review**: All unsafe code must be reviewed for soundness
2. **Attack Surface Analysis**: Identify and minimize kernel entry points
3. **Fuzzing**: Use fuzzing tools to test input validation
4. **Security Testing**: Write explicit tests for security-critical paths
5. **Documentation**: Maintain clear security documentation for changes

### Example: Secure Setuid Binary

```rust
// Properly drop privileges after initialization
fn secure_main() -> Result<()> {
    // Initialize with root privileges
    let config = read_config("/etc/sensitive.conf")?;
    
    // Drop privileges immediately
    unsafe {
        libc::seteuid(unsafe { libc::getuid() })?;
    }
    
    // Now running with original UID
    process_untrusted_input(&config)?;
    Ok(())
}
```

### Example: Safe Signal Handling

```rust
// Volatile access prevents optimizer transformations
volatile_write(flag, true);

// Signal handlers should be minimal and re-entrant
fn signal_handler(sig: i32) {
    // Safe: only volatile writes and syscalls
    volatile_write(signal_count, volatile_read(signal_count) + 1);
}
```

## Vulnerability Reporting

### Reporting Process

If you discover a security vulnerability in Qunix:

1. **Do NOT disclose publicly**: Report privately to the maintainers
2. **Email Security Team**: Contact security@qunix-os.local (when established)
3. **Include Details**:
   - Vulnerability type and severity assessment
   - Affected components and versions
   - Steps to reproduce
   - Proof of concept (if available)
4. **Expected Response**: Acknowledgment within 48 hours; fix target within 30 days
5. **Responsible Disclosure**: Allow time for patch before public disclosure

### CVE Process

Once fixed, vulnerabilities will be:
- Assigned a CVE identifier
- Published in a security advisory
- Credited to the reporter (unless requested otherwise)

## Security Roadmap

### Short Term (0-6 months)

- [ ] Implement Linux seccomp syscall filtering
- [ ] Add seccomp-based system call whitelist support
- [ ] Stack canary support in userland binaries
- [ ] Improve ASLR coverage for all address spaces

### Medium Term (6-18 months)

- [ ] Full Linux namespace support (user, pid, ipc, uts, network)
- [ ] AppArmor/SELinux-style MAC implementation
- [ ] Kernel hardening: CFI, CET, shadow stacks
- [ ] Secure boot with UEFI/SecureBoot support
- [ ] Cryptographic verification for kernel modules

### Long Term (18+ months)

- [ ] Formal verification of critical kernel paths
- [ ] Hypervisor support for nested virtualization
- [ ] Hardware memory tagging (MTE) support
- [ ] Full audit subsystem (CONFIG_AUDIT)
- [ ] Trusted Platform Module (TPM) integration
- [ ] Real-time security monitoring framework

## Security Contact

For security-related questions or concerns:

- **Documentation**: See [Security.md](Security.md) (this file)
- **Issues**: Report non-security bugs on GitHub Issues
- **Security**: Report vulnerabilities privately (process TBD)
- **Contributing**: See [Developer Guide](doc/developer_guide.md)

## References

- [Rust Security Guidelines](https://anssi-fr.github.io/rust-guide/)
- [Linux Security Subsystem Documentation](https://www.kernel.org/doc/html/latest/security/)
- [POSIX Security Model](https://pubs.opengroup.org/onlinepubs/9699919799/basedefs/V1_chap04.html)
- [CWE/OWASP Top 10](https://owasp.org/www-project-top-ten/)
- [The System Design of the UNIX Operating System](https://www.usenix.org/system/files/login/articles/10_020_u6000_082715_online.pdf)

---

**Last Updated**: April 2026  
**Version**: 1.0  
**Status**: Under Active Development
