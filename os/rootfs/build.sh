#!/bin/bash
# EnerOS minimal rootfs build script
# Creates a minimal rootfs based on musl libc with statically linked Rust binaries
set -euo pipefail

# Configuration
ARCH="${ARCH:-x86_64}"
TARGET_TRIPLE="${TARGET_TRIPLE:-}"
OUTPUT_DIR="${OUTPUT_DIR:-$(pwd)/output}"
ROOTFS_DIR="${ROOTFS_DIR:-$OUTPUT_DIR/rootfs}"
ROOTFS_TARBALL="${ROOTFS_TARBALL:-$OUTPUT_DIR/eneros-rootfs-$ARCH.tar.gz}"

# Determine Rust target triple
if [ -z "$TARGET_TRIPLE" ]; then
    case "$ARCH" in
        x86_64)  TARGET_TRIPLE="x86_64-unknown-linux-musl" ;;
        aarch64) TARGET_TRIPLE="aarch64-unknown-linux-musl" ;;
        *) echo "Unsupported arch: $ARCH"; exit 1 ;;
    esac
fi

echo "=== EnerOS Rootfs Build ==="
echo "Architecture: $ARCH"
echo "Target triple: $TARGET_TRIPLE"
echo "Output dir: $OUTPUT_DIR"
echo "Rootfs dir: $ROOTFS_DIR"

# Create directories
mkdir -p "$ROOTFS_DIR"

# Step 1: Create directory structure
echo ">>> Creating directory structure..."
mkdir -p "$ROOTFS_DIR"/{bin,sbin,etc,proc,sys,dev,run,tmp,var/lib/eneros,var/log/eneros,var/tmp,home/eneros,root,lib,usr/bin,usr/sbin,usr/lib}
chmod 1777 "$ROOTFS_DIR/tmp" "$ROOTFS_DIR/var/tmp"

# Step 2: Build EnerOS binaries (statically linked)
echo ">>> Building EnerOS binaries (static linking)..."

# Ensure musl target is installed
rustup target add "$TARGET_TRIPLE" 2>/dev/null || true

# Build with static linking
export RUSTFLAGS="-C target-feature=+crt-static"
cargo build --release --target "$TARGET_TRIPLE" -p eneros-api
cargo build --release --target "$TARGET_TRIPLE" -p eneros-init

# Step 3: Install binaries
echo ">>> Installing binaries..."
cp "target/$TARGET_TRIPLE/release/eneros-api" "$ROOTFS_DIR/bin/eneros-api"
cp "target/$TARGET_TRIPLE/release/eneros-init" "$ROOTFS_DIR/bin/eneros-init"
chmod 755 "$ROOTFS_DIR/bin/eneros-api" "$ROOTFS_DIR/bin/eneros-init"

# Step 4: Install configuration files
echo ">>> Installing configuration files..."
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Copy /etc files
cp "$SCRIPT_DIR/files/etc/passwd" "$ROOTFS_DIR/etc/passwd"
cp "$SCRIPT_DIR/files/etc/group" "$ROOTFS_DIR/etc/group"
cp "$SCRIPT_DIR/files/etc/hostname" "$ROOTFS_DIR/etc/hostname"
cp "$SCRIPT_DIR/files/etc/eneros/init.toml" "$ROOTFS_DIR/etc/eneros/init.toml"

# Create symlinks for init
ln -sf /bin/eneros-init "$ROOTFS_DIR/sbin/init"
ln -sf /bin/eneros-init "$ROOTFS_DIR/bin/init"

# Step 5: Create minimal /etc files
echo ">>> Creating system files..."

# /etc/os-release
cat > "$ROOTFS_DIR/etc/os-release" << 'EOF'
NAME="EnerOS"
VERSION="1.0.0"
ID=eneros
ID_LIKE=linux
PRETTY_NAME="EnerOS Power-Native OS 1.0.0"
VERSION_ID="1.0"
HOME_URL="https://github.com/eneros/eneros"
SUPPORT_URL="https://github.com/eneros/eneros/issues"
BUG_REPORT_URL="https://github.com/eneros/eneros/issues"
EOF

# /etc/hosts
cat > "$ROOTFS_DIR/etc/hosts" << 'EOF'
127.0.0.1   localhost
::1         localhost ip6-localhost ip6-loopback
EOF

# /etc/resolv.conf (will be managed by eneros-netcfg)
cat > "$ROOTFS_DIR/etc/resolv.conf" << 'EOF'
nameserver 8.8.8.8
nameserver 8.8.4.4
EOF

# /etc/nsswitch.conf
cat > "$ROOTFS_DIR/etc/nsswitch.conf" << 'EOF'
passwd: files
group: files
shadow: files
hosts: files dns
networks: files
EOF

# Step 6: Create device nodes (will be created by devtmpfs at runtime,
# but create essential ones for initramfs)
echo ">>> Creating device nodes..."
mknod -m 622 "$ROOTFS_DIR/dev/console" c 5 1 2>/dev/null || true
mknod -m 666 "$ROOTFS_DIR/dev/null" c 1 3 2>/dev/null || true
mknod -m 666 "$ROOTFS_DIR/dev/zero" c 1 5 2>/dev/null || true
mknod -m 666 "$ROOTFS_DIR/dev/ptmx" c 5 2 2>/dev/null || true
mknod -m 666 "$ROOTFS_DIR/dev/tty" c 5 0 2>/dev/null || true
mknod -m 444 "$ROOTFS_DIR/dev/random" c 1 8 2>/dev/null || true
mknod -m 444 "$ROOTFS_DIR/dev/urandom" c 1 9 2>/dev/null || true

# Step 7: Set permissions
echo ">>> Setting permissions..."
chown -R 0:0 "$ROOTFS_DIR"
chmod 700 "$ROOTFS_DIR/root"
chmod 755 "$ROOTFS_DIR/home/eneros"

# Step 8: Calculate size
echo ">>> Calculating rootfs size..."
ROOTFS_SIZE=$(du -sh "$ROOTFS_DIR" | cut -f1)
echo "Rootfs size: $ROOTFS_SIZE"

# Step 9: Create tarball
echo ">>> Creating rootfs tarball..."
tar -czf "$ROOTFS_TARBALL" -C "$ROOTFS_DIR" .

echo "=== Rootfs build complete ==="
echo "Rootfs: $ROOTFS_DIR"
echo "Tarball: $ROOTFS_TARBALL"
echo "Size: $ROOTFS_SIZE"
