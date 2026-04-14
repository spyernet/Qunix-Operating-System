#!/bin/bash
# Qunix OS — Dependency Setup Script
# Run this ONCE to install all required build tools.
set -e

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[1;33m'
CYAN='\033[0;36m'; NC='\033[0m'

info()  { echo -e "${CYAN}[INFO]${NC} $*"; }
ok()    { echo -e "${GREEN}[OK]${NC}   $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
die()   { echo -e "${RED}[FAIL]${NC} $*"; exit 1; }

echo -e "${CYAN}"
echo "  ██████╗ ██╗   ██╗███╗   ██╗██╗██╗  ██╗"
echo "  ██╔═══██╗██║   ██║████╗  ██║██║╚██╗██╔╝"
echo "  ██║   ██║██║   ██║██╔██╗ ██║██║ ╚███╔╝ "
echo "  ██║▄▄ ██║██║   ██║██║╚██╗██║██║ ██╔██╗ "
echo "  ╚██████╔╝╚██████╔╝██║ ╚████║██║██╔╝ ██╗"
echo "   ╚══▀▀═╝  ╚═════╝ ╚═╝  ╚═══╝╚═╝╚═╝  ╚═╝"
echo -e "${NC}"
echo "  Qunix OS Build Setup"
echo "  ===================="
echo ""

# 1. Detect OS
if [ -f /etc/debian_version ] || [ -f /etc/ubuntu_version ]; then
    PKG_MGR="apt"
elif [ -f /etc/arch-release ]; then
    PKG_MGR="pacman"
elif [ -f /etc/fedora-release ] || [ -f /etc/redhat-release ]; then
    PKG_MGR="dnf"
elif command -v brew &>/dev/null; then
    PKG_MGR="brew"
else
    PKG_MGR="unknown"
fi
info "Detected package manager: $PKG_MGR"

# 2. System packages 
info "Installing system dependencies..."
case "$PKG_MGR" in
  apt)
    sudo apt-get update -q
    sudo apt-get install -y \
      lld llvm binutils \
      mtools xorriso dosfstools \
      qemu-system-x86_64 qemu-utils \
      curl git make nasm
    ;;
  pacman)
    sudo pacman -Syu --noconfirm \
      lld llvm binutils \
      mtools libisoburn \
      qemu-system-x86_64 \
      curl git make nasm
    ;;
  dnf)
    sudo dnf install -y \
      lld llvm binutils \
      mtools xorriso \
      qemu-system-x86_64 \
      curl git make nasm
    ;;
  brew)
    brew install llvm mtools xorriso nasm qemu
    # Add LLVM to path
    export PATH="$(brew --prefix llvm)/bin:$PATH"
    ;;
  *)
    warn "Unknown package manager. Install manually:"
    warn "  - lld (LLVM linker)"
    warn "  - mtools (FAT filesystem tools)"
    warn "  - xorriso (ISO creation)"
    warn "  - qemu-system-x86_64"
    warn "  - nasm"
    ;;
esac
ok "System packages installed"

# 3. Rust toolchain
if ! command -v rustup &>/dev/null; then
    info "Installing Rust via rustup..."
    curl https://sh.rustup.rs -sSf | sh -s -- -y
    source "$HOME/.cargo/env"
else
    info "rustup already installed"
fi

# Ensure nightly + required targets/components
info "Installing nightly toolchain..."
rustup toolchain install nightly
rustup default nightly
rustup target add x86_64-unknown-none
rustup component add rust-src rustfmt clippy llvm-tools-preview
ok "Rust nightly toolchain ready"

# 4. Verify
echo ""
info "Verifying installation..."
MISSING=0
for tool in cargo rustc ld.lld mtools qemu-system-x86_64; do
    if command -v $tool &>/dev/null; then
        echo "  ✓  $tool ($(command -v $tool))"
    else
        echo "  ✗  $tool MISSING"
        MISSING=$((MISSING+1))
    fi
done

echo ""
if [ $MISSING -eq 0 ]; then
    ok "All dependencies satisfied!"
    echo ""
    echo "  Build with:    ./build.sh kernel"
    echo "  Run in QEMU:   ./build.sh run"
    echo "  Full ISO:      ./build.sh iso"
else
    warn "$MISSING tool(s) missing. Check output above."
    exit 1
fi
