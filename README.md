![OS logo](visuals/logo.png)
# Qunix OS

A Unix-like operating system kernel written in Rust for everyone.

## Features

- **Full process model** — fork, exec, wait, exit, signals, job control
- **VFS layer** — ext4, FAT32, tmpfs, devfs, procfs
- **Pipe I/O** — blocking pipes with proper EOF/EPIPE semantics
- **TTY subsystem** — canonical mode, line discipline, Ctrl+C/Z, termios
- **Signal delivery** — full POSIX signals, sigaction, sigreturn, sigprocmask
- **Memory management** — COW fork, mmap/munmap/mremap/brk, demand paging
- **ELF loader** — static and dynamic binaries, interpreter support
- **Futex** — WAIT/WAKE/REQUEUE/WAKE_OP, correct blocking semantics
- **Scheduler** — priority-based, preemptive, multi-CPU ready
- **Userland** — 70+ userland utilities (shell, ls, cat, grep, sed, awk, ...)

## Quick Start

### 1. Install dependencies

```bash
./SETUP.sh
```

Or manually:
```bash
# Ubuntu/Debian
sudo apt-get install lld mtools xorriso qemu-system-x86_64 curl
curl https://sh.rustup.rs -sSf | sh -s -- -y
source $HOME/.cargo/env
rustup toolchain install nightly
rustup component add rust-src
```

### 2. Build

```bash
# Build kernel only
./build.sh kernel

# Build everything + create bootable ISO
./build.sh iso

# Build and run in QEMU
./build.sh run
```

### 3. QEMU options

```bash
# Default: 512MB RAM, 1 CPU, serial output
./build.sh run

# More RAM, more CPUs
QEMU_MEMORY=2G QEMU_CPUS=4 ./build.sh run
```

## Documentation

Comprehensive documentation is available in the `doc/` directory:

- **[Overview](doc/overview.md)** — High-level project description and features
- **[Architecture](doc/architecture.md)** — Detailed kernel and system architecture
- **[Building](doc/building.md)** — Build system and compilation guide
- **[User Guide](doc/user_guide.md)** — Usage instructions and system administration
- **[Developer Guide](doc/developer_guide.md)** — Contributing and development information

## Build Requirements

| Tool | Version | Purpose |
|------|---------|---------|
| rustup | any | Rust toolchain manager |
| rustc nightly | ≥ 2024-01 | Kernel compilation (`build-std`) |
| ld.lld | ≥ 14 | Linker (kernel target requires LLD) |
| mtools | any | FAT filesystem creation |
| xorriso | any | ISO creation |
| qemu-system-x86_64 | ≥ 7.0 | Emulation |

## Project Structure

```
qunix/
├── kernel/          # Rust kernel (this is where 90% of work lives)
│   ├── src/
│   │   ├── main.rs          # Kernel entry point
│   │   ├── arch/x86_64/     # CPU, GDT, IDT, paging, syscall entry
│   │   ├── memory/          # VMM, CoW, frame allocator
│   │   ├── process/         # Process table, fork, clone, PCB
│   │   ├── sched/           # Scheduler (priority queues, blocking)
│   │   ├── signal/          # Signal delivery, sigreturn, sigframe
│   │   ├── syscall/         # Syscall dispatch + all handlers
│   │   ├── vfs/             # Virtual filesystem layer
│   │   ├── fs/              # ext4, FAT32, tmpfs, devfs
│   │   ├── ipc/             # Pipes, shared memory, futex
│   │   ├── elf/             # ELF64 loader (static + dynamic)
│   │   ├── tty/             # TTY line discipline, termios
│   │   ├── time/            # PIT timer, sleep, RTC
│   │   ├── drivers/         # Keyboard, VGA, serial, IRQ
│   │   └── abi_compat/      # Linux ABI compatibility layer
│   ├── x86_64-qunix.json    # Custom Rust target spec
│   └── kernel.ld            # Linker script
├── bootloader/      # UEFI bootloader
├── userland/        # 70+ userland programs (shell, coreutils)
├── plugins/         # Loadable kernel modules
├── configs/         # GRUB config, UEFI startup script
├── build.sh         # Main build system
├── SETUP.sh         # Dependency installer
└── README.md        # This file
```

## Recent Changes

### Initial Release (v0.2.0)
- Complete kernel implementation with full process model
- 70+ userland utilities including modern shell (qsh)
- Multiple filesystem support (ext4, FAT32, tmpfs, devfs, procfs)
- Networking stack with TCP/UDP
- Plugin system for loadable kernel modules
- Security features: namespaces, memory tagging, seccomp
- Advanced IPC: pipes, shared memory, futexes
- Comprehensive documentation added
*Please Note that the project structure info may be not updated*

## Kernel Architecture

### Memory Layout (x86-64)

```
0x0000_0000_0000_0000  User space start
0x0000_0001_0000_0000  Heap start (brk)
0x0000_4000_0000_0000  mmap base
0x0000_5000_0000_0000  PIE executable base
0x0000_7FFF_0000_0000  Dynamic linker (ld.so)
0x0000_7FFF_FFFF_0000  User stack top
─────────────────────  Canonical hole
0xFFFF_8000_0000_0000  Kernel virtual base
0xFFFF_C000_0000_0000  Kernel heap
0xFFFF_FFFF_FFFF_FFFF  Top of address space
```

### Syscall ABI

Standard Linux x86-64 syscall ABI:
- `syscall` instruction, `sysretq` return
- `rax` = syscall number, args in `rdi rsi rdx r10 r8 r9`
- Return value in `rax`

Implemented: ~250 syscalls (all major POSIX + Linux-specific)

## Userland Programs

The `userland/` directory contains implementations of:

**Shell:** `qshell` — a POSIX-compatible shell with pipelines, job control,
variables, redirection, and builtins.

**Core utilities:** ls, cat, echo, grep, sed, awk, find, sort, uniq, wc,
head, tail, cut, tr, tee, xargs, du, df, ps, top, kill, sleep, date,
mkdir, rm, cp, mv, ln, chmod, chown, stat, touch, ...

Each utility is a standalone Rust binary targeting `x86_64-qunix-user`.

## Development Notes

### Adding a syscall

1. Add handler in `kernel/src/syscall/handlers.rs`
2. Wire in dispatch table in `kernel/src/syscall/mod.rs`
3. Add to `kernel/src/abi_compat/abi/mod.rs` if it needs constants

### Adding a filesystem

1. Implement `SuperblockOps` + `InodeOps` traits in `kernel/src/fs/`
2. Register in `kernel/src/fs/mod.rs` init()
3. Mount in VFS

### Signal delivery flow

```
send_signal(pid, sig)
  → p.sig_pending.add(sig)
  → wake_process(pid) if sleeping

[at syscall exit in dispatch()]
  → deliver_pending_at_syscall_exit(frame)
    → for each pending signal:
      User handler: build_signal_frame() → patch frame.rip_saved
                    set_user_rsp(new_rsp)
      Default:      apply_default() [terminate/stop/ignore]

[handler runs, calls sigreturn (syscall 15)]
  → sigreturn(frame)
    → read SigFrame from get_user_rsp()
    → restore all regs into SyscallFrame
    → set_user_rsp(mc.rsp)
    → return mc.rax
```

## License

Qunix Operating System is licensed under the terms of MIT licence.
