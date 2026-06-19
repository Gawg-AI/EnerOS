#!/bin/bash
# EnerOS Linux kernel build script
# Builds Linux kernel with PREEMPT_RT patch for EnerOS Power-Native OS
set -euo pipefail

# Configuration
KERNEL_VERSION="${KERNEL_VERSION:-6.6}"
RT_PATCH_VERSION="${RT_PATCH_VERSION:-6.6-rt23}"
ARCH="${ARCH:-x86_64}"
JOBS="${JOBS:-$(nproc)}"
BUILD_DIR="${BUILD_DIR:-/tmp/eneros-kernel-build}"
OUTPUT_DIR="${OUTPUT_DIR:-$(pwd)/output}"

# Map arch to kernel arch
case "$ARCH" in
    x86_64)  KERNEL_ARCH="x86" ;;
    aarch64) KERNEL_ARCH="arm64" ;;
    *) echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

echo "=== EnerOS Kernel Build ==="
echo "Kernel version: $KERNEL_VERSION"
echo "RT patch version: $RT_PATCH_VERSION"
echo "Architecture: $ARCH ($KERNEL_ARCH)"
echo "Build dir: $BUILD_DIR"
echo "Output dir: $OUTPUT_DIR"

# Create directories
mkdir -p "$BUILD_DIR" "$OUTPUT_DIR"

# Step 1: Download kernel source
KERNEL_TARBALL="linux-$KERNEL_VERSION.tar.xz"
KERNEL_URL="https://cdn.kernel.org/pub/linux/kernel/v${KERNEL_VERSION%%.*}.x/$KERNEL_TARBALL"
if [ ! -f "$BUILD_DIR/$KERNEL_TARBALL" ]; then
    echo ">>> Downloading kernel source..."
    wget -q -O "$BUILD_DIR/$KERNEL_TARBALL" "$KERNEL_URL"
fi

# Step 2: Extract kernel source
KERNEL_SRC_DIR="$BUILD_DIR/linux-$KERNEL_VERSION"
if [ ! -d "$KERNEL_SRC_DIR" ]; then
    echo ">>> Extracting kernel source..."
    tar -xf "$BUILD_DIR/$KERNEL_TARBALL" -C "$BUILD_DIR"
fi

# Step 3: Download and apply PREEMPT_RT patch
RT_PATCH="patch-$RT_PATCH_VERSION.patch.gz"
RT_PATCH_URL="https://cdn.kernel.org/pub/linux/kernel/projects/rt/${KERNEL_VERSION%%.*}.x/$RT_PATCH"
if [ ! -f "$BUILD_DIR/$RT_PATCH" ]; then
    echo ">>> Downloading PREEMPT_RT patch..."
    wget -q -O "$BUILD_DIR/$RT_PATCH" "$RT_PATCH_URL"
fi

# Step 4: Apply RT patch
echo ">>> Applying PREEMPT_RT patch..."
cd "$KERNEL_SRC_DIR"
if ! zcat "$BUILD_DIR/$RT_PATCH" | patch -p1 --dry-run > /dev/null 2>&1; then
    echo "RT patch already applied or failed to apply"
else
    zcat "$BUILD_DIR/$RT_PATCH" | patch -p1
fi

# Step 5: Copy config
echo ">>> Copying kernel config..."
CONFIG_FILE="$(dirname "$0")/config-$ARCH"
if [ ! -f "$CONFIG_FILE" ]; then
    echo "Config file not found: $CONFIG_FILE"
    exit 1
fi
cp "$CONFIG_FILE" "$KERNEL_SRC_DIR/.config"

# Step 6: Configure kernel
echo ">>> Configuring kernel..."
make ARCH=$KERNEL_ARCH olddefconfig

# Step 7: Build kernel
echo ">>> Building kernel (this may take a while)..."
make ARCH=$KERNEL_ARCH -j"$JOBS" bzImage modules

# Step 8: Install to output
echo ">>> Installing kernel to output..."
mkdir -p "$OUTPUT_DIR/boot" "$OUTPUT_DIR/lib/modules"

# Copy kernel image
if [ "$ARCH" = "x86_64" ]; then
    cp arch/x86/boot/bzImage "$OUTPUT_DIR/boot/vmlinuz-eneros"
elif [ "$ARCH" = "aarch64" ]; then
    cp arch/arm64/boot/Image "$OUTPUT_DIR/boot/vmlinuz-eneros"
fi

# Install modules
make ARCH=$KERNEL_ARCH INSTALL_MOD_PATH="$OUTPUT_DIR" modules_install

# Copy config and System.map
cp .config "$OUTPUT_DIR/boot/config-eneros"
cp System.map "$OUTPUT_DIR/boot/System.map-eneros"

echo "=== Kernel build complete ==="
echo "Output: $OUTPUT_DIR/boot/vmlinuz-eneros"
echo "Modules: $OUTPUT_DIR/lib/modules/"
