#!/usr/bin/env bash
set -e
cd "$(dirname "${BASH_SOURCE[0]}")/kernel"

export PATH="$HOME/.cargo/bin:$PATH"
SYSROOT=$(rustc --print sysroot)

echo "Building Qunix kernel..."
RUSTC_BOOTSTRAP=1 \
RUSTFLAGS="--sysroot $SYSROOT \
  -C relocation-model=static \
  -C code-model=kernel \
  -C target-feature=-mmx,-sse,-red-zone \
  -C link-arg=-Tlinker.ld \
  -C link-arg=-e \
  -C link-arg=kernel_main \
  -C link-arg=--no-gc-sections" \
cargo build --release --offline --target x86_64-unknown-none

KERNEL=target/x86_64-unknown-none/release/kernel
echo ""
echo "=== Build complete ==="
echo "Binary:      $(ls -lh $KERNEL | awk '{print $5, $9}')"
echo "Entry point: $(nm $KERNEL | awk '/T kernel_main/{print $1}')"
