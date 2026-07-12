#!/usr/bin/env bash
# ============================================================
# EnerOS / Power Native Agent OS — Toolchain Setup Script
# Version: v0.1.0
# Target: WSL2 Ubuntu-22.04 (or native Linux x86_64)
# ============================================================
# Installs all required tools for cross-compiling seL4 + Rust for ARM64.
# Idempotent: safe to run multiple times.
# ============================================================

set -euo pipefail

# Color output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info()  { echo -e "${GREEN}[INFO]${NC} $*"; }
warn()  { echo -e "${YELLOW}[WARN]${NC} $*"; }
error() { echo -e "${RED}[ERROR]${NC} $*" >&2; }

# Check if running on WSL2 or Linux
if [[ "$(uname -r)" != *microsoft* ]] && [[ "$(uname -s)" != "Linux" ]]; then
    error "This script must be run on WSL2 or native Linux."
    exit 1
fi

# Rust nightly version (locked)
RUST_NIGHTLY="nightly-2026-04-04"
RUST_TARGET="aarch64-unknown-none"

# seL4 build dependencies (apt packages)
APT_PACKAGES=(
    "cmake"
    "ninja-build"
    "device-tree-compiler"
    "gdb-multiarch"
    "gcc-aarch64-linux-gnu"
    "qemu-system-aarch64"
    "llvm"
    "clang"
    "libxml2-utils"
    "python3"
    "python3-pip"
)

info "=== EnerOS Toolchain Setup (v0.1.0) ==="
info "Rust nightly: ${RUST_NIGHTLY}"
info "Target: ${RUST_TARGET}"
echo ""

# --- Step 1: Update apt and install system packages ---
info "Step 1: Installing system packages via apt..."

# Check if packages are already installed
NEED_INSTALL=false
for pkg in "${APT_PACKAGES[@]}"; do
    if ! dpkg -s "$pkg" &>/dev/null; then
        NEED_INSTALL=true
        break
    fi
done

if [[ "$NEED_INSTALL" == "true" ]]; then
    sudo apt-get update -y
    for pkg in "${APT_PACKAGES[@]}"; do
        if ! dpkg -s "$pkg" &>/dev/null; then
            info "  Installing: $pkg"
            sudo apt-get install -y "$pkg"
        else
            info "  Already installed: $pkg"
        fi
    done
else
    info "  All apt packages already installed."
fi

echo ""

# --- Step 2: Install Rust via rustup ---
info "Step 2: Installing Rust via rustup..."

# Install rustup if not present
if ! command -v rustup &>/dev/null; then
    info "  Installing rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain "${RUST_NIGHTLY}"
    source "${HOME}/.cargo/env"
else
    info "  rustup already installed."
fi

# Source cargo environment
if [[ -f "${HOME}/.cargo/env" ]]; then
    source "${HOME}/.cargo/env"
fi

# Install the locked nightly toolchain
if ! rustup toolchain list | grep -q "${RUST_NIGHTLY}"; then
    info "  Installing Rust ${RUST_NIGHTLY}..."
    rustup toolchain install "${RUST_NIGHTLY}" --component rust-src,clippy,rustfmt
else
    info "  Rust ${RUST_NIGHTLY} already installed."
fi

# Set default toolchain
rustup default "${RUST_NIGHTLY}"

# Add the aarch64-unknown-none target
if ! rustup target list --installed | grep -q "${RUST_TARGET}"; then
    info "  Adding target: ${RUST_TARGET}"
    rustup target add "${RUST_TARGET}"
else
    info "  Target ${RUST_TARGET} already added."
fi

# Ensure rust-src is available (required for build-std)
if ! rustup component list --installed | grep -q "rust-src"; then
    info "  Adding component: rust-src"
    rustup component add rust-src
else
    info "  Component rust-src already added."
fi

# Ensure clippy is available
if ! rustup component list --installed | grep -q "clippy"; then
    info "  Adding component: clippy"
    rustup component add clippy
else
    info "  Component clippy already added."
fi

# Ensure rustfmt is available
if ! rustup component list --installed | grep -q "rustfmt"; then
    info "  Adding component: rustfmt"
    rustup component add rustfmt
else
    info "  Component rustfmt already added."
fi

echo ""

# --- Step 3: Install cargo-audit ---
info "Step 3: Installing cargo-audit..."
if ! cargo audit --version &>/dev/null; then
    info "  Installing cargo-audit..."
    cargo install cargo-audit
else
    info "  cargo-audit already installed."
fi

echo ""

# --- Step 4: Verify installations ---
info "Step 4: Verifying installations..."

verify() {
    local cmd="$1"
    local name="$2"
    if command -v "$cmd" &>/dev/null; then
        info "  ✓ $name: $($cmd --version 2>&1 | head -1)"
    else
        error "  ✗ $name: NOT FOUND"
        return 1
    fi
}

verify rustc "Rust compiler"
verify cargo "Cargo"
verify rustup "rustup"
verify aarch64-linux-gnu-gcc "ARM64 GCC"
verify qemu-system-aarch64 "QEMU (ARM64)"
verify cmake "CMake"
verify ninja "Ninja"
verify dtc "Device Tree Compiler"
verify gdb-multiarch "GDB Multiarch"
verify llvm-objcopy "LLVM objcopy"
verify llvm-objdump "LLVM objdump"
verify cargo-audit "cargo-audit"

echo ""

# --- Step 5: Print toolchain matrix ---
info "=== Toolchain Matrix ==="
info "  Rust:          $(rustc --version)"
info "  Cargo:         $(cargo --version)"
info "  Rustup:        $(rustup --version)"
info "  ARM64 GCC:     $(aarch64-linux-gnu-gcc --version | head -1)"
info "  QEMU:          $(qemu-system-aarch64 --version | head -1)"
info "  CMake:         $(cmake --version | head -1)"
info "  Ninja:         $(ninja --version)"
info "  DTC:           $(dtc --version)"
info "  GDB:           $(gdb-multiarch --version | head -1)"
info "  Target:        ${RUST_TARGET}"
info "  Nightly:       ${RUST_NIGHTLY}"
echo ""

info "=== Setup Complete! ==="
info "You can now build EnerOS with: make build"
info "You can now run QEMU with:    make run"
