#!/bin/bash
# EnerOS initramfs build script
# Creates a minimal initramfs containing eneros-init and essential kernel modules
set -euo pipefail

ARCH="${ARCH:-x86_64}"
OUTPUT_DIR="${OUTPUT_DIR:-$(pwd)/output}"
KERNEL_OUTPUT="${KERNEL_OUTPUT:-../kernel/output}"
ROOTFS_OUTPUT="${ROOTFS_OUTPUT:-../rootfs/output}"
INITRAMFS="${INITRAMFS:-$OUTPUT_DIR/initramfs.img}"

echo "=== EnerOS Initramfs Build ==="
echo "Architecture: $ARCH"
echo "Output: $INITRAMFS"

# Determine kernel arch
case "$ARCH" in
    x86_64)  KERNEL_ARCH="x86" ;;
    aarch64) KERNEL_ARCH="arm64" ;;
    *) echo "Unsupported arch: $ARCH"; exit 1 ;;
esac

# Create temporary initramfs directory
INITRAMFS_DIR=$(mktemp -d)
trap "rm -rf $INITRAMFS_DIR" EXIT

echo ">>> Creating initramfs structure..."
mkdir -p "$INITRAMFS_DIR"/{bin,sbin,etc,proc,sys,dev,run,tmp,mnt/root,usr/bin,usr/sbin,lib,lib64}
chmod 1777 "$INITRAMFS_DIR/tmp"

# Copy eneros-init binary from rootfs
if [ -d "$ROOTFS_OUTPUT/rootfs" ]; then
    cp "$ROOTFS_OUTPUT/rootfs/bin/eneros-init" "$INITRAMFS_DIR/bin/eneros-init"
    cp "$ROOTFS_OUTPUT/rootfs/bin/eneros-api" "$INITRAMFS_DIR/bin/eneros-api"
    chmod 755 "$INITRAMFS_DIR/bin/eneros-init" "$INITRAMFS_DIR/bin/eneros-api"
else
    echo "ERROR: rootfs not found at $ROOTFS_OUTPUT/rootfs"
    echo "Please run os/rootfs/build.sh first"
    exit 1
fi

# Create init script (the first process in initramfs)
cat > "$INITRAMFS_DIR/init" << 'INITEOF'
#!/bin/sh
# EnerOS initramfs init script
# Mounts essential filesystems and exec's eneros-init

echo "EnerOS initramfs starting..."

# Mount essential filesystems
mount -t proc none /proc
mount -t sysfs none /sys
mount -t devtmpfs none /dev
mount -t tmpfs none /run
mount -t tmpfs none /tmp

# Load essential kernel modules (if any)
# modprobe ext4
# modprobe virtio_pci
# modprobe virtio_net
# modprobe virtio_blk

# Find the root partition
# In production, this would scan for the EnerOS root partition
# For now, try common device names
ROOT_DEVICE=""
for dev in /dev/sda2 /dev/vda2 /dev/nvme0n1p2; do
    if [ -b "$dev" ]; then
        ROOT_DEVICE="$dev"
        break
    fi
done

if [ -n "$ROOT_DEVICE" ]; then
    echo "Mounting root filesystem from $ROOT_DEVICE..."
    mount -t ext4 "$ROOT_DEVICE" /mnt/root
    
    if [ -x /mnt/root/bin/eneros-init ]; then
        echo "Switching to real root..."
        # Move mounted filesystems to the real root
        mount --move /proc /mnt/root/proc
        mount --move /sys /mnt/root/sys
        mount --move /dev /mnt/root/dev
        mount --move /run /mnt/root/run
        mount --move /tmp /mnt/root/tmp
        
        # Switch root and exec eneros-init
        exec switch_root /mnt/root /bin/eneros-init
    else
        echo "ERROR: eneros-init not found on root filesystem"
        echo "Dropping to emergency shell..."
        exec /bin/sh
    fi
else
    echo "ERROR: No root device found"
    echo "Available devices:"
    ls -la /dev/sd* /dev/vd* /dev/nvme* 2>/dev/null || echo "  none"
    echo "Dropping to emergency shell..."
    exec /bin/sh
fi
INITEOF
chmod 755 "$INITRAMFS_DIR/init"

# Create minimal /etc files
cat > "$INITRAMFS_DIR/etc/passwd" << 'EOF'
root:x:0:0:root:/root:/bin/sh
EOF

cat > "$INITRAMFS_DIR/etc/group" << 'EOF'
root:x:0:
EOF

# Copy kernel modules (if available)
if [ -d "$KERNEL_OUTPUT/lib/modules" ]; then
    echo ">>> Copying kernel modules..."
    KVER=$(ls "$KERNEL_OUTPUT/lib/modules" | head -1)
    if [ -n "$KVER" ]; then
        mkdir -p "$INITRAMFS_DIR/lib/modules/$KVER"
        # Copy only essential modules
        cp -a "$KERNEL_OUTPUT/lib/modules/$KVER/kernel/drivers/virtio" \
              "$INITRAMFS_DIR/lib/modules/$KVER/" 2>/dev/null || true
        cp -a "$KERNEL_OUTPUT/lib/modules/$KVER/kernel/drivers/net" \
              "$INITRAMFS_DIR/lib/modules/$KVER/" 2>/dev/null || true
        cp -a "$KERNEL_OUTPUT/lib/modules/$KVER/kernel/fs/ext4" \
              "$INITRAMFS_DIR/lib/modules/$KVER/" 2>/dev/null || true
        # Copy modules.dep and modules.builtin
        cp "$KERNEL_OUTPUT/lib/modules/$KVER/modules.dep" \
           "$INITRAMFS_DIR/lib/modules/$KVER/" 2>/dev/null || true
        cp "$KERNEL_OUTPUT/lib/modules/$KVER/modules.builtin" \
           "$INITRAMFS_DIR/lib/modules/$KVER/" 2>/dev/null || true
    fi
fi

# Create device nodes
mknod -m 622 "$INITRAMFS_DIR/dev/console" c 5 1 2>/dev/null || true
mknod -m 666 "$INITRAMFS_DIR/dev/null" c 1 3 2>/dev/null || true
mknod -m 666 "$INITRAMFS_DIR/dev/zero" c 1 5 2>/dev/null || true
mknod -m 666 "$INITRAMFS_DIR/dev/tty" c 5 0 2>/dev/null || true

# Pack initramfs
echo ">>> Packing initramfs..."
cd "$INITRAMFS_DIR"
find . | cpio -H newc -o | gzip -9 > "$INITRAMFS"

INITRAMFS_SIZE=$(du -sh "$INITRAMFS" | cut -f1)
echo "=== Initramfs build complete ==="
echo "File: $INITRAMFS"
echo "Size: $INITRAMFS_SIZE"
