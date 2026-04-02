#!/usr/bin/env bash
# Qunix OS Build System v5.0
# Usage: ./build.sh [bin|iso|run|clean|kernel|userland|plugins|check|help]
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BUILD_DIR="$SCRIPT_DIR/build"
KERNEL_SRC="$SCRIPT_DIR/kernel"
BOOTLOADER_SRC="$SCRIPT_DIR/bootloader"
USERLAND_SRC="$SCRIPT_DIR/userland"
PLUGINS_SRC="$SCRIPT_DIR/plugins"
DISK_IMG="$BUILD_DIR/qunix.img"
ISO_IMG="$BUILD_DIR/qunix.iso"
KERNEL_ELF="$BUILD_DIR/kernel.elf"
BOOTLOADER_EFI="$BUILD_DIR/bootx64.efi"

VERBOSE=${VERBOSE:-false}
QEMU_MEMORY=${QEMU_MEMORY:-512M}
QEMU_CPUS=${QEMU_CPUS:-1}
KERNEL_TOOLCHAIN=${KERNEL_TOOLCHAIN:-nightly}
BOOTLOADER_TOOLCHAIN=${BOOTLOADER_TOOLCHAIN:-nightly-2023-12-01}

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
BLUE='\033[0;34m'; CYAN='\033[0;36m'; NC='\033[0m'

info()    { echo -e "${BLUE}[INFO]${NC}    $*"; }
success() { echo -e "${GREEN}[OK]${NC}      $*"; }
warn()    { echo -e "${YELLOW}[WARN]${NC}    $*"; }
error()   { echo -e "${RED}[ERROR]${NC}   $*" >&2; }
step()    { echo -e "\n${CYAN}━━━ $* ━━━${NC}"; }

# ── Dependency check ──────────────────────────────────────────────────────

check_deps() {
    local missing=()
    command -v cargo  >/dev/null || missing+=("cargo (via rustup)")
    command -v rustup >/dev/null || missing+=("rustup")
    command -v ld.lld >/dev/null || missing+=("lld (llvm-lld package)")
    command -v mtools >/dev/null || missing+=("mtools")
    command -v dd     >/dev/null || missing+=("dd (coreutils)")
    command -v qemu-system-x86_64 >/dev/null 2>&1 \
        || warn "qemu-system-x86_64 not found (needed for 'run' command)"

    if [ ${#missing[@]} -ne 0 ]; then
        error "Missing required tools: ${missing[*]}"
        exit 1
    fi

    # Kernel toolchain (latest nightly) and components.
    rustup toolchain install "$KERNEL_TOOLCHAIN" --component rust-src rustfmt clippy \
        >/dev/null 2>&1 || true

    # Bootloader toolchain is pinned because uefi 0.18 is not compatible with
    # newer nightly compilers yet.
    rustup toolchain install "$BOOTLOADER_TOOLCHAIN" --component rust-src \
        >/dev/null 2>&1 || true
    rustup target add x86_64-unknown-uefi --toolchain "$BOOTLOADER_TOOLCHAIN" \
        >/dev/null 2>&1 || true
}

# ── Plugin code generation ────────────────────────────────────────────────
#
# Parses plugins/*/main.conf and generates kernel/src/plugins/generated.rs
# which is a Rust source file that:
#   1. include!()s each plugin's entry .rs file as a sub-module
#   2. Defines register_all() that registers every plugin with the kernel

build_plugins() {
    step "Processing plugins"
    local gen_file="$KERNEL_SRC/src/plugins/generated.rs"
    local plugins_found=0
    local plugin_modules=""
    local plugin_registers=""
    local plugin_hook_count=0

    if [ ! -d "$PLUGINS_SRC" ]; then
        warn "No plugins/ directory found — skipping plugin generation"
        write_empty_generated "$gen_file"
        return 0
    fi

    # Parse each plugin directory
    for plugin_dir in "$PLUGINS_SRC"/*/; do
        [ -d "$plugin_dir" ] || continue
        local conf="$plugin_dir/main.conf"
        [ -f "$conf" ] || { warn "Plugin dir $(basename $plugin_dir): missing main.conf, skipping"; continue; }

        # Parse main.conf fields
        local pname pversion pauthor plicense pdesc pentry penabled
        pname=""    ; pversion="0.0"; pauthor="Unknown"
        plicense="" ; pdesc=""      ; pentry=""; penabled="true"

        while IFS='=' read -r key value; do
            # Trim whitespace
            key="${key// /}"; value="${value// /}"
            case "$key" in
                name)        pname="$value" ;;
                version)     pversion="$value" ;;
                author)      pauthor="$value" ;;
                license)     plicense="$value" ;;
                description) pdesc="$value" ;;
                entry)       pentry="$value" ;;
                enabled)     penabled="$value" ;;
            esac
        done < "$conf"

        if [ -z "$pname" ]; then
            warn "Plugin in $(basename $plugin_dir): 'name' missing in main.conf, skipping"
            continue
        fi
        if [ -z "$pentry" ]; then
            warn "Plugin '$pname': 'entry' missing in main.conf, skipping"
            continue
        fi

        local entry_path="$plugin_dir/$pentry"
        if [ ! -f "$entry_path" ]; then
            warn "Plugin '$pname': entry file '$pentry' not found at $entry_path, skipping"
            continue
        fi

        info "Plugin: $pname v$pversion ($plicense) — $pdesc"

        # Sanitize name for Rust module identifier (replace - with _)
        local mod_name="${pname//-/_}"
        # Absolute path for include!()
        local abs_entry
        # Use a path relative to kernel/src/plugins/ (where generated.rs lives)
        # so include!() resolves correctly on any machine
        abs_entry="$(realpath --relative-to="$KERNEL_SRC/src/plugins" "$entry_path")"

        plugin_modules+="
mod ${mod_name} {
    include!(\"${abs_entry}\");
}"

        local boot_enabled="false"
        [ "$penabled" = "true" ] && boot_enabled="true"

        plugin_registers+="
    crate::plugins::register(&${mod_name}::PLUGIN_ENTRY);"
        plugin_hook_count=$((plugin_hook_count + 1))
        plugins_found=$((plugins_found + 1))
    done

    # Write generated.rs
    cat > "$gen_file" << RUST_EOF
//! Auto-generated plugin registry — DO NOT EDIT
//! Generated by build.sh from plugins/*/main.conf
//! Regenerate by running: ./build.sh plugins (or any full build)
//!
//! Plugins compiled in: ${plugins_found}
//! To add a plugin:   create plugins/<n>/main.conf + <entry>.rs, rebuild kernel
//! To remove:         delete plugins/<n>/, rebuild kernel
//! To enable/disable: pluginctl enable/disable <n>  (no rebuild needed)
${plugin_modules}

/// Register all compiled-in plugins with the kernel plugin manager.
/// Called once from kernel_main before plugins::init().
pub fn register_all() {
${plugin_registers}

    // Sync HOOKS_ACTIVE counter with the number of initially-enabled plugins
    let enabled_count = crate::plugins::list()
        .iter()
        .filter(|(_name, enabled, _ver, _desc)| *enabled)
        .count() as u32;
    crate::plugins::hooks::HOOKS_ACTIVE.store(
        enabled_count,
        core::sync::atomic::Ordering::Relaxed,
    );
}
RUST_EOF

    success "Plugins: $plugins_found compiled in, generated $gen_file"
}

write_empty_generated() {
    local gen_file="$1"
    cat > "$gen_file" << 'RUST_EOF'
//! Auto-generated plugin registry (empty — no plugins/ directory found)
pub fn register_all() {}
RUST_EOF
}

# ── Kernel build ──────────────────────────────────────────────────────────

build_kernel() {
    step "Building kernel"
    mkdir -p "$BUILD_DIR"

    # Generate plugin registry BEFORE building kernel
    build_plugins

    local flags="--release"
    local kernel_rustflags="${RUSTFLAGS:+$RUSTFLAGS }-Awarnings"
    $VERBOSE && flags="$flags -v"

    (
        cd "$KERNEL_SRC"
        RUSTFLAGS="$kernel_rustflags" cargo +"$KERNEL_TOOLCHAIN" build $flags \
            --target x86_64-qunix.json -Zjson-target-spec \
            -Z build-std=core,compiler_builtins,alloc \
            -Z build-std-features=compiler-builtins-mem \
            2>&1
    )

    cp "$KERNEL_SRC/target/x86_64-qunix/release/kernel" "$KERNEL_ELF"
    success "Kernel: $KERNEL_ELF ($(du -sh "$KERNEL_ELF" | cut -f1))"
}

# ── Bootloader build ──────────────────────────────────────────────────────

build_bootloader() {
    step "Building UEFI bootloader"
    mkdir -p "$BUILD_DIR"
    (
        cd "$BOOTLOADER_SRC"
        cargo +"$BOOTLOADER_TOOLCHAIN" -Znext-lockfile-bump build --release 2>&1
    )
    cp "$BOOTLOADER_SRC/target/x86_64-unknown-uefi/release/bootloader.efi" "$BOOTLOADER_EFI"
    success "Bootloader: $BOOTLOADER_EFI"
}

# ── Userland build ────────────────────────────────────────────────────────
#
# Reads the USERLAND_DIRS list and builds each program.
# pluginctl is always included.

USERLAND_DIRS=(
    "awk" "b64encode" "basename" "cat" "chmod" "chown"
    "column" "comm" "cp" "cut" "date" "dd" "df"
    "dirname" "dmesg" "du" "echo" "env" "expand"
    "expr" "false" "find" "fold" "free" "grep" "head"
    "hostname" "id" "init" "kill" "less" "ln" "ls"
    "lscpu" "md5sum" "mkdir" "mkfifo" "mktemp"
    "more" "mv" "nl" "nproc" "paste" "pgrep"
    "pluginctl" "printf" "ps" "pwd" "qshell" "readlink"
    "realpath" "rev" "rm" "sed" "seq" "sha256sum"
    "shuf" "sleep" "sort" "stat" "sync" "tac"
    "tail" "tee" "time" "timeout" "touch" "tr" "true"
    "tty" "uname" "uniq" "uptime" "wait" "wc"
    "which" "whoami" "xargs" "yes"
)

build_userland() {
    step "Building userland (${#USERLAND_DIRS[@]} programs)"
    local rootfs="$BUILD_DIR/rootfs"
    local userland_target="$BUILD_DIR/userland-target"
    mkdir -p "$rootfs/bin" "$rootfs/sbin" "$rootfs/usr/bin" "$rootfs/usr/sbin"
    mkdir -p "$userland_target"

    local failed=0 built=0

    for dir_name in "${USERLAND_DIRS[@]}"; do
        local src_dir="$USERLAND_SRC/$dir_name"
        [ -d "$src_dir" ] || { warn "Missing userland dir: $dir_name"; continue; }
        local cargo_toml="$src_dir/Cargo.toml"

        # Determine binary output name from [[bin]] first, then [package].
        local bin_name
        bin_name=$(
            awk -F'"' '
                /^\[\[bin\]\]/ { in_bin=1; next }
                /^\[/ { if (in_bin) exit }
                in_bin && $1 ~ /^[[:space:]]*name[[:space:]]*=/ { print $2; exit }
            ' "$cargo_toml"
        )
        if [ -z "$bin_name" ]; then
            bin_name=$(
                awk -F'"' '
                    /^\[package\]/ { in_pkg=1; next }
                    /^\[/ { if (in_pkg) exit }
                    in_pkg && $1 ~ /^[[:space:]]*name[[:space:]]*=/ { print $2; exit }
                ' "$cargo_toml"
            )
        fi
        [ -z "$bin_name" ] && { warn "Could not determine bin name: $dir_name"; failed=$((failed+1)); continue; }

        local cargo_flags="--release"
        $VERBOSE && cargo_flags="$cargo_flags -v"

        if (
            cd "$src_dir"
            RUSTFLAGS="-C link-arg=-T${USERLAND_SRC}/user.ld -C relocation-model=static" \
            CARGO_TARGET_DIR="$userland_target" \
            cargo +nightly build $cargo_flags \
                --target "${USERLAND_SRC}/x86_64-qunix-user.json" -Zjson-target-spec \
                -Z build-std=core,compiler_builtins,alloc \
                -Z build-std-features=compiler-builtins-mem
        ); then
            local bin_out="$userland_target/x86_64-qunix-user/release/${bin_name}.elf"
            if [ ! -f "$bin_out" ]; then
                bin_out=$(find "$userland_target/x86_64-qunix-user/release/" \
                    -maxdepth 1 -type f -name "${bin_name}*" -executable 2>/dev/null | head -1)
            fi
            if [ -n "$bin_out" ] && [ -f "$bin_out" ]; then
                cp "$bin_out" "$rootfs/bin/$bin_name"
                if [ "$bin_name" = "init" ]; then
                    cp "$bin_out" "$rootfs/sbin/init"
                fi
                built=$((built+1))
            else
                warn "No executable produced: $dir_name"
                failed=$((failed+1))
            fi
        else
            warn "Failed to build: $dir_name"
            failed=$((failed+1))
        fi
    done

    # FAT images don't preserve host symlinks, so materialize these aliases.
    if [ -f "$rootfs/bin/qshell" ]; then
        rm -f "$rootfs/bin/sh" "$rootfs/bin/bash" "$rootfs/bin/qsh"
        cp -f "$rootfs/bin/qshell" "$rootfs/bin/sh"
        cp -f "$rootfs/bin/qshell" "$rootfs/bin/bash"
        cp -f "$rootfs/bin/qshell" "$rootfs/bin/qsh"
    fi

    # Populate /etc
    populate_etc "$rootfs"

    success "Userland: $built built, $failed failed"
}

populate_etc() {
    local rootfs="$1"
    mkdir -p "$rootfs/etc" "$rootfs/root" "$rootfs/tmp"
    mkdir -p "$rootfs/proc" "$rootfs/sys" "$rootfs/dev" "$rootfs/run"

    cat > "$rootfs/etc/passwd" << 'EOF'
root:x:0:0:root:/root:/bin/qshell
nobody:x:65534:65534:nobody:/:/bin/false
EOF
    cat > "$rootfs/etc/group" << 'EOF'
root:x:0:
nobody:x:65534:
EOF
    cat > "$rootfs/etc/hostname" << 'EOF'
qunix
EOF
    cat > "$rootfs/etc/os-release" << 'EOF'
NAME="Qunix"
VERSION="5.0"
ID=qunix
PRETTY_NAME="Qunix OS 5.0"
VERSION_ID="5.0"
BUILD_ID=v5
ANSI_COLOR="1;32"
EOF
    cat > "$rootfs/etc/motd" << 'EOF'

  ██████╗ ██╗   ██╗███╗   ██╗██╗██╗  ██╗  v5
 ██╔═══██╗██║   ██║████╗  ██║██║╚██╗██╔╝
 ██║   ██║██║   ██║██╔██╗ ██║██║ ╚███╔╝
 ╚██████╔╝╚██████╔╝██║ ╚████║██║██╔╝ ██╗
  ╚═════╝  ╚═════╝ ╚═╝  ╚═══╝╚═╝╚═╝  ╚═╝

Qunix OS 5.0 — Plugin-capable Rust OS
Type 'pluginctl list' to see active plugins.

EOF
    cat > "$rootfs/etc/profile" << 'EOF'
export PATH=/bin:/sbin:/usr/bin:/usr/sbin
export HOME=/root
export TERM=xterm-256color
export LANG=en_US.UTF-8
export PS1='\u@\h:\w\$ '
EOF
    cat > "$rootfs/etc/fstab" << 'EOF'
tmpfs  /     tmpfs  defaults  0 0
devfs  /dev  devfs  defaults  0 0
proc   /proc proc   defaults  0 0
sysfs  /sys  sysfs  defaults  0 0
EOF
}

# ── Disk image assembly ───────────────────────────────────────────────────

build_disk_image() {
    step "Creating disk image: $DISK_IMG"
    local rootfs="$BUILD_DIR/rootfs"
    local EFI_SZ_MB=64
    local DATA_SZ_MB=512
    local DISK_SZ_MB=$((EFI_SZ_MB + DATA_SZ_MB + 2))
    local tmp_img_dir
    tmp_img_dir="$(mktemp -d)"

    cleanup_build_disk_image() {
        rm -rf "$tmp_img_dir"
    }
    trap cleanup_build_disk_image RETURN

    mkdir -p "$BUILD_DIR"

    if command -v truncate >/dev/null 2>&1; then
        truncate -s "${DISK_SZ_MB}M" "$DISK_IMG"
    else
        dd if=/dev/zero of="$DISK_IMG" bs=1M count=$DISK_SZ_MB status=none
    fi

    local EFI_START=$((2048))
    local EFI_SIZE=$((EFI_SZ_MB * 2048))
    local DATA_START=$((EFI_START + EFI_SIZE))
    local DATA_SIZE=$((DATA_SZ_MB * 2048))

    # EFI partition (FAT32)
    local efi_img="$tmp_img_dir/efi.img"
    dd if=/dev/zero of="$efi_img" bs=1M count=$EFI_SZ_MB status=none
    mformat -i "$efi_img" -F -v "QUNIX_EFI" ::
    mmd -i "$efi_img" ::/EFI
    mmd -i "$efi_img" ::/EFI/BOOT
    mmd -i "$efi_img" ::/EFI/QUNIX
    mcopy -i "$efi_img" "$BOOTLOADER_EFI" ::/EFI/BOOT/BOOTX64.EFI
    mcopy -i "$efi_img" "$KERNEL_ELF"     ::/EFI/QUNIX/KERNEL.ELF
    [ -f "$rootfs/sbin/init" ] && \
        mcopy -i "$efi_img" "$rootfs/sbin/init" ::/EFI/QUNIX/INIT.ELF
    [ -f "$rootfs/bin/qshell" ] && \
        mcopy -i "$efi_img" "$rootfs/bin/qshell" ::/EFI/QUNIX/QSHELL.ELF
    [ -f "$SCRIPT_DIR/configs/startup.nsh" ] && \
        mcopy -i "$efi_img" "$SCRIPT_DIR/configs/startup.nsh" ::/startup.nsh

    # Data partition (FAT32, rootfs)
    local data_img="$tmp_img_dir/data.img"
    dd if=/dev/zero of="$data_img" bs=1M count=$DATA_SZ_MB status=none
    mformat -i "$data_img" -F -v "QUNIX_ROOT" ::

    if [ -d "$rootfs" ]; then
        copy_rootfs_to_fat "$data_img" "$rootfs"
        success "Data partition: $(find "$rootfs" -type f | wc -l) files"
    fi

    # Partition table
    printf "n\np\n1\n%d\n%d\nt\nef\nn\np\n2\n%d\n%d\nw\n" \
        "$EFI_START" "$((EFI_START + EFI_SIZE - 1))" \
        "$DATA_START" "$((DATA_START + DATA_SIZE - 1))" \
        | fdisk "$DISK_IMG" >/dev/null 2>&1 || true

    dd if="$efi_img"  of="$DISK_IMG" bs=512 seek=$EFI_START  count=$EFI_SIZE  conv=notrunc status=none
    dd if="$data_img" of="$DISK_IMG" bs=512 seek=$DATA_START count=$DATA_SIZE conv=notrunc status=none

    success "Disk image: $DISK_IMG ($(du -sh "$DISK_IMG" | cut -f1))"
}

copy_rootfs_to_fat() {
    local img="$1" src="$2"
    # Create directory tree
    find "$src" -mindepth 1 -type d | sort | while read -r dir; do
        local rel="${dir#$src}"
        mmd -i "$img" "::${rel}" 2>/dev/null || true
    done
    # Copy files
    find "$src" -type f | while read -r file; do
        local rel="${file#$src}"
        MTOOLS_SKIP_CHECK=1 mcopy -i "$img" -o "$file" "::${rel}" 2>/dev/null || true
    done
}

# ── QEMU launcher ─────────────────────────────────────────────────────────

run_qemu() {
    step "Launching Qunix in QEMU"
    command -v qemu-system-x86_64 >/dev/null || { error "qemu-system-x86_64 not found"; exit 1; }

    local ovmf=""
    for f in /usr/share/OVMF/OVMF_CODE.fd /usr/share/ovmf/OVMF.fd \
             /usr/share/edk2/x64/OVMF_CODE.fd /usr/share/edk2-ovmf/OVMF_CODE.fd; do
        [ -f "$f" ] && { ovmf="$f"; break; }
    done

    local qargs=(
        -machine q35
        -cpu qemu64,+sse,+sse2,+ssse3,+sse4.1,+sse4.2
        -m "$QEMU_MEMORY"
        -smp "$QEMU_CPUS"
        -drive "file=$DISK_IMG,format=raw,if=ide"
        -net nic,model=virtio -net user
        -rtc base=localtime
        -no-reboot
        -serial stdio
    )
    [ -n "$ovmf" ]  && qargs+=(-bios "$ovmf") || warn "OVMF not found — install ovmf package"
    [ "${QEMU_DISPLAY:-}" = "none" ] && qargs+=(-display none) || qargs+=(-vga std)

    info "QEMU args: ${qargs[*]}"
    exec qemu-system-x86_64 "${qargs[@]}"
}

# ── Clean ─────────────────────────────────────────────────────────────────

do_clean() {
    step "Cleaning build artifacts"
    rm -rf "$BUILD_DIR"
    (cd "$KERNEL_SRC"     && cargo clean 2>/dev/null || true)
    (cd "$BOOTLOADER_SRC" && cargo clean 2>/dev/null || true)
    find "$USERLAND_SRC" -name "Cargo.toml" -maxdepth 2 -exec bash -c \
        'cd "$(dirname "$1")" && cargo clean 2>/dev/null || true' _ {} \;
    success "Clean complete"
}

do_check() {
    step "Type-checking kernel"
    build_plugins  # regenerate first
    local kernel_rustflags="${RUSTFLAGS:+$RUSTFLAGS }-Awarnings"
    (
        cd "$KERNEL_SRC"
        RUSTFLAGS="$kernel_rustflags" cargo +"$KERNEL_TOOLCHAIN" check \
            --target x86_64-qunix.json -Zjson-target-spec \
            -Z build-std=core,compiler_builtins,alloc \
            -Z build-std-features=compiler-builtins-mem
    )
    success "Check passed"
}

print_usage() {
    echo "Qunix OS Build System v5.0"
    echo ""
    echo "Usage: $0 COMMAND"
    echo ""
    echo "Commands:"
    echo "  iso        Build complete bootable disk image (default)"
    echo "  run        Build and launch in QEMU"
    echo "  bin        Build kernel + userland (no disk image)"
    echo "  kernel     Build kernel only"
    echo "  userland   Build userland only"
    echo "  plugins    Regenerate plugin registry only"
    echo "  check      Type-check kernel without full build"
    echo "  clean      Remove all build artifacts"
    echo "  help       Show this help"
    echo ""
    echo "Environment:"
    echo "  VERBOSE=true           Verbose build output"
    echo "  QEMU_MEMORY=512M       QEMU memory"
    echo "  QEMU_CPUS=1            QEMU CPU count"
    echo "  QEMU_DISPLAY=none      Headless QEMU"
    echo ""
    echo "Plugin development:"
    echo "  1. Create plugins/<n>/main.conf"
    echo "  2. Write plugins/<n>/plug/<entry>.rs"
    echo "  3. Run ./build.sh kernel"
    echo "  4. After boot: pluginctl enable/disable <n>"
}

# ── Main dispatch ─────────────────────────────────────────────────────────

CMD="${1:-iso}"
case "$CMD" in
    iso|disk)
        check_deps
        build_kernel
        build_bootloader
        build_userland
        build_disk_image
        success "Qunix v5 disk image ready: $DISK_IMG"
        ;;
    run)
        check_deps
        build_kernel
        build_bootloader
        build_userland
        build_disk_image
        run_qemu
        ;;
    bin)
        check_deps
        build_kernel
        build_bootloader
        build_userland
        success "Qunix v5 binaries ready"
        ;;
    kernel)
        check_deps
        build_kernel
        ;;
    bootloader)
        check_deps
        build_bootloader
        ;;
    userland)
        check_deps
        build_userland
        ;;
    plugins)
        build_plugins
        ;;
    check)
        check_deps
        do_check
        ;;
    clean)
        do_clean
        ;;
    help|--help|-h)
        print_usage
        ;;
    *)
        error "Unknown command: $CMD"
        print_usage
        exit 1
        ;;
esac
