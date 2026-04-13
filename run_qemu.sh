#!/usr/bin/env bash
#
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
KERNEL_NEW="$SCRIPT_DIR/build/kernel.elf"
KERNEL_OLD="$SCRIPT_DIR/kernel/target/x86_64-unknown-none/release/kernel"
DISK_IMG="$SCRIPT_DIR/build/qunix.img"
BOOTLOADER_EFI="$SCRIPT_DIR/build/bootx64.efi"
ROOTFS_DIR="$SCRIPT_DIR/build/rootfs"
KERNEL="$KERNEL_NEW"
 
latest_mtime() {
    local latest=0
    local path
    for path in "$@"; do
        [ -e "$path" ] || continue
        local mtime
        mtime=$(stat -c %Y "$path" 2>/dev/null || echo 0)
        if [ "$mtime" -gt "$latest" ]; then
            latest="$mtime"
        fi
    done
    echo "$latest"
}

rootfs_mtime() {
    if [ ! -d "$ROOTFS_DIR" ]; then
        echo 0
        return
    fi
    find "$ROOTFS_DIR" -type f -printf '%T@\n' 2>/dev/null \
        | awk 'BEGIN { max = 0 } { if ($1 > max) max = $1 } END { printf "%.0f\n", max }'
}

ensure_fresh_disk_image() {
    [ "${QUNIX_SKIP_IMAGE_REFRESH:-0}" = "1" ] && return

    local disk_mtime=0
    if [ -f "$DISK_IMG" ]; then
        disk_mtime=$(stat -c %Y "$DISK_IMG" 2>/dev/null || echo 0)
    fi

    local deps_mtime
    deps_mtime=$(latest_mtime "$KERNEL_NEW" "$KERNEL_OLD" "$BOOTLOADER_EFI")
    local rootfs_latest
    rootfs_latest=$(rootfs_mtime)
    if [ "$rootfs_latest" -gt "$deps_mtime" ]; then
        deps_mtime="$rootfs_latest"
    fi

    if [ ! -f "$DISK_IMG" ] || [ "$deps_mtime" -gt "$disk_mtime" ]; then
        echo "Refreshing disk image before boot..."
        bash "$SCRIPT_DIR/build.sh" iso
    fi
}

if [ ! -f "$KERNEL" ] && [ -f "$KERNEL_OLD" ]; then
    KERNEL="$KERNEL_OLD"
fi

if [ ! -f "$KERNEL" ] && [ ! -f "$DISK_IMG" ]; then
    echo "ERROR: no boot artifact found:"
    echo "  $DISK_IMG"
    echo "  $KERNEL_NEW"
    echo "  $KERNEL_OLD"
    echo "Build first: ./build.sh run  (or ./build.sh iso / ./build.sh kernel)"
    exit 1
fi

# Check QEMU
if ! command -v qemu-system-x86_64 &>/dev/null; then
    echo "ERROR: qemu-system-x86_64 not found"
    echo "Install: sudo apt-get install qemu-system-x86 seabios"
    exit 1
fi

ensure_fresh_disk_image

if [ -f "$DISK_IMG" ]; then
    OVMF=""
    for f in /usr/share/OVMF/OVMF.fd \
             /usr/share/OVMF/OVMF.4m.fd \
             /usr/share/OVMF/x64/OVMF.fd \
             /usr/share/OVMF/x64/OVMF.4m.fd \
             /usr/share/ovmf/OVMF.fd \
             /usr/share/ovmf/OVMF.4m.fd \
             /usr/share/ovmf/x64/OVMF.fd \
             /usr/share/ovmf/x64/OVMF.4m.fd \
             /usr/share/edk2/OVMF.fd \
             /usr/share/edk2/OVMF.4m.fd \
             /usr/share/edk2/x64/OVMF.fd \
             /usr/share/edk2/x64/OVMF.4m.fd \
             /usr/share/edk2-ovmf/OVMF.fd \
             /usr/share/edk2-ovmf/OVMF.4m.fd \
             /usr/share/edk2-ovmf/x64/OVMF.fd \
             /usr/share/edk2-ovmf/x64/OVMF.4m.fd \
             /usr/share/OVMF/OVMF_CODE.fd \
             /usr/share/OVMF/OVMF_CODE.4m.fd \
             /usr/share/OVMF/x64/OVMF_CODE.fd \
             /usr/share/OVMF/x64/OVMF_CODE.4m.fd \
             /usr/share/ovmf/OVMF_CODE.fd \
             /usr/share/ovmf/OVMF_CODE.4m.fd \
             /usr/share/ovmf/x64/OVMF_CODE.fd \
             /usr/share/ovmf/x64/OVMF_CODE.4m.fd \
             /usr/share/edk2/x64/OVMF_CODE.fd \
             /usr/share/edk2/x64/OVMF_CODE.4m.fd \
             /usr/share/edk2-ovmf/OVMF_CODE.fd \
             /usr/share/edk2-ovmf/OVMF_CODE.4m.fd \
             /usr/share/edk2-ovmf/x64/OVMF_CODE.fd \
             /usr/share/edk2-ovmf/x64/OVMF_CODE.4m.fd; do
        [ -f "$f" ] && { OVMF="$f"; break; }
    done
    if [ -n "$OVMF" ]; then
        echo "=== Booting Qunix OS (UEFI disk) ==="
        echo "Disk:   $(ls -lh "$DISK_IMG" | awk '{print $5, $9}')"
        echo "QEMU:   $(qemu-system-x86_64 --version | head -1)"
        echo "OVMF:   $OVMF"
        echo ""

        QEMU_AUDIO_DRV=none qemu-system-x86_64 \
            -machine q35 \
            -m 512M \
            -cpu qemu64 \
            -drive "file=$DISK_IMG,format=raw,if=ide" \
            -bios "$OVMF" \
            -serial stdio \
            -display none \
            -audio none \
            -no-reboot \
            -net none \
            "$@"
        exit $?
    fi

    echo "WARN: OVMF firmware not found; falling back to direct kernel boot."
    if [ ! -f "$KERNEL" ]; then
        echo "ERROR: no kernel fallback available at $KERNEL"
        echo "Install OVMF (sudo apt-get install ovmf) or build kernel artifact."
        exit 1
    fi
fi

echo "=== Booting Qunix OS ==="
echo "Kernel: $(ls -lh $KERNEL | awk '{print $5, $9}')"
echo "QEMU:   $(qemu-system-x86_64 --version | head -1)"
echo ""

TMPDIR=""
cleanup() {
    if [ -n "$TMPDIR" ] && [ -d "$TMPDIR" ]; then
        rm -rf "$TMPDIR"
    fi
}
trap cleanup EXIT

# Build PVH trampoline if objcopy and as available
# (allows direct -kernel boot in QEMU 8.x)
if command -v as &>/dev/null && command -v objcopy &>/dev/null; then
    KM=$(nm "$KERNEL" | awk '/T kernel_main/{print "0x"$1}')
    if [ -z "$KM" ]; then
        echo "WARN: kernel_main symbol not found; skipping PVH trampoline injection."
    else
        TMPDIR=$(mktemp -d)
    
        # 32-bit setup: enable long mode, jump to 0x100600
        cat > "$TMPDIR/t32.S" << 'ASM'
.code32
.org 0
_s: cli
    lgdt gp-_s+0x100400
    mov %cr4,%ecx; or $0x20,%ecx; mov %ecx,%cr4
    mov $0x101000,%ecx; mov %ecx,%cr3
    mov $0xC0000080,%ecx; rdmsr; or $0x100,%eax; wrmsr
    mov %cr0,%ecx; or $0x80000001,%ecx; mov %ecx,%cr0
    ljmp $0x08,$0x100600
.align 8
g: .quad 0; .quad 0x00af9a000000ffff; .quad 0x00cf92000000ffff
ge:
gp: .word ge-g-1; .long g-_s+0x100400
ASM

        # 64-bit stub: call kernel_main
        python3 -c "
import struct
km = $KM
code = bytes([
    0x66,0xb8,0x10,0x00,          # mov \$0x10,%ax
    0x8e,0xd8,0x8e,0xc0,0x8e,0xd0, # mov %ax,%ds/%es/%ss
    0x31,0xc0,0x8e,0xe0,0x8e,0xe8, # xor; mov %ax,%fs/%gs
    0x48,0xbc,0x00,0x00,0x20,0x00,0x00,0x00,0x00,0x00, # mov \$0x200000,%rsp
    0x48,0x31,0xff,                # xor %rdi,%rdi
    0x48,0xb8] + list(struct.pack('<Q',km)) + [  # movabs km,%rax
    0xff,0xd0,                     # call *%rax
    0xf4,0xeb,0xfd])               # hlt; jmp
open('$TMPDIR/s64.bin','wb').write(code)
"

        # Assemble trampoline
        if as --32 "$TMPDIR/t32.S" -o "$TMPDIR/t32.o" 2>/dev/null && \
           ld -m elf_i386 -Ttext=0x100400 --oformat binary -o "$TMPDIR/t32.bin" "$TMPDIR/t32.o" 2>/dev/null; then
            python3 -c "
import struct, os
b = bytearray(0x3000)
t = open('$TMPDIR/t32.bin','rb').read()
s = open('$TMPDIR/s64.bin','rb').read()
b[0:len(t)]=t; b[0x200:0x200+len(s)]=s
b[0x1000:0x1008]=struct.pack('<Q',0x102000|3)
for i in range(4): b[0x2000+i*8:0x2000+i*8+8]=struct.pack('<Q',(i<<30)|0x83)
open('$TMPDIR/pvh.bin','wb').write(bytes(b))
"
            # Build PVH note
            python3 -c "
import struct
note = struct.pack('<III',4,4,18) + b'Xen\x00' + struct.pack('<I',0x100400)
open('$TMPDIR/pvh_note.bin','wb').write(note)
"
            # Inject into kernel; only switch kernels if both objcopy steps succeed.
            PVHKERNEL="$TMPDIR/kernel_pvh"
            cp "$KERNEL" "$PVHKERNEL"
            if objcopy --add-section .pvh_tramp="$TMPDIR/pvh.bin" \
                       --set-section-flags .pvh_tramp=alloc,load \
                       --change-section-address .pvh_tramp=0x100400 "$PVHKERNEL" 2>/dev/null && \
               objcopy --add-section ".note.Xen.PVH=$TMPDIR/pvh_note.bin" \
                       --set-section-flags ".note.Xen.PVH=alloc,load" \
                       --change-section-address ".note.Xen.PVH=0x100000" "$PVHKERNEL" 2>/dev/null; then
                KERNEL="$PVHKERNEL"
                echo "PVH trampoline injected -> $(basename "$KERNEL")"
            else
                echo "WARN: PVH injection failed; using original kernel image."
            fi
        fi
    fi
fi

QEMU_AUDIO_DRV=none qemu-system-x86_64 \
    -machine pc \
    -m 256M \
    -cpu qemu64 \
    -kernel "$KERNEL" \
    -serial stdio \
    "$@"
