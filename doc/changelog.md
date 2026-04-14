# Qunix Changelog

All notable changes to Qunix will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - 2026-04-02

### Added
- **Complete Kernel Implementation**
  - Full process model with fork/exec/wait/exit
  - Preemptive priority-based scheduler
  - Memory management with CoW fork and demand paging
  - Virtual filesystem layer with multiple backends
  - Signal delivery system with POSIX semantics
  - System call interface with 100+ syscalls
  - Device driver framework

- **Userland Environment**
  - 70+ userland utilities (coreutils, shell, text processing)
  - Modern interactive shell (qsh) with advanced features
  - Init system for system startup
  - Comprehensive standard library (libsys)

- **Filesystems**
  - ext4 with journaling support
  - FAT32 for external media
  - tmpfs for temporary storage
  - devfs for device nodes
  - procfs for process information

- **Networking**
  - TCP/UDP protocol implementation
  - Socket API
  - IP networking

- **IPC Mechanisms**
  - Pipes with proper blocking semantics
  - Shared memory
  - Message queues
  - Semaphores
  - Futexes for fast userspace locking

- **Security Features**
  - Process namespaces
  - Memory tagging
  - Seccomp system call filtering
  - MAC policies

- **Plugin System**
  - Loadable kernel modules
  - Runtime plugin management
  - Example plugins (perf_monitor, syscall_logger)

- **Advanced Features**
  - DRM/KMS graphics support
  - I/O Uring for high-performance I/O
  - Transparent huge pages
  - RTOS scheduling classes
  - ELF loader with dynamic linking

- **Build System**
  - Comprehensive build scripts
  - Cross-compilation support
  - Automated testing infrastructure

- **Documentation**
  - Complete user and developer documentation
  - Architecture overview
  - Build and usage guides

### Technical Details
- **Language**: Rust (no_std kernel, custom targets)
- **Architecture**: x86-64
- **Bootloader**: UEFI with GRUB compatibility
- **Testing**: QEMU-based emulation and testing
- **Code Quality**: Memory safe, well-documented, tested

## [0.1.0] - Initial Development
- Project initialization
- Basic kernel structure
- Basic bootloader