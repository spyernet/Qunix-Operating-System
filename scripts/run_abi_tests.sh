#!/bin/bash
# Run the ABI validation suite
# Tests kernel syscall behavior using a statically-compiled test binary
set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

echo "Building ABI test suite..."
gcc -static -O2 -o /tmp/qunix_abi_test "$SCRIPT_DIR/abi_test.c"

echo "Running on native Linux (ground truth)..."
/tmp/qunix_abi_test

echo ""
echo "Running under QEMU user-mode..."
"$SCRIPT_DIR/qemu-amd64-static" /tmp/qunix_abi_test
