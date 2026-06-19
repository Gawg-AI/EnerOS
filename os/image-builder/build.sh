#!/bin/bash
# EnerOS image builder
# 创建可启动的 raw 镜像，支持 UEFI 启动，5 分区 A/B 布局
set -euo pipefail

# Configuration
ARCH="${ARCH:-x86_64}"
OUTPUT_DIR="${OUTPUT_DIR:-$(pwd)/output}"
IMAGE_NAME="${IMAGE_NAME:-eneros-$ARCH.img}"
IMAGE_SIZE="${IMAGE_SIZE:-4G}"
EFI_SIZE="${EFI_SIZE:-512M}"
ROOT_SIZE="${ROOT_SIZE:-1536M}"
CONFIG_SIZE="${CONFIG_SIZE:-256M}"

# Paths to dependencies
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OS_DIR="$(dirname "$SCRIPT_DIR")"
KERNEL_DIR="$OS_DIR/kernel"
ROOTFS_DIR="$OS_DIR/rootfs"
BOOT_DIR="$OS_DIR/boot"

# 默认机器配置文件路径
MACHINE_CONFIG="${MACHINE_CONFIG:-$ROOTFS_DIR/files/etc/eneros/eneros-machine.yaml}"

# 解析命令行参数
while [ $# -gt 0 ]; do
    case "$1" in
        --machine-config)
            MACHINE_CONFIG="$2"
            shift 2
            ;;
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --image-size)
            IMAGE_SIZE="$2"
            shift 2
            ;;
        --help|-h)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --machine-config PATH  机器配置文件路径（默认: rootfs/files/etc/eneros/eneros-machine.yaml）"
            echo "  --arch ARCH            目标架构：x86_64 或 aarch64（默认: x86_64）"
            echo "  --image-size SIZE      镜像大小（默认: 4G）"
            echo ""
            echo "Environment variables:"
            echo "  ARCH, OUTPUT_DIR, IMAGE_NAME, IMAGE_SIZE, EFI_SIZE, ROOT_SIZE, CONFIG_SIZE, MACHINE_CONFIG"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

IMAGE="$OUTPUT_DIR/$IMAGE_NAME"

echo "=== EnerOS Image Builder ==="
echo "Architecture: $ARCH"
echo "Image: $IMAGE"
echo "Size: $IMAGE_SIZE"
echo "Machine config: $MACHINE_CONFIG"
echo "Partition layout: 5-partition A/B (EFI / RootA / RootB / Data / Config)"

# 验证机器配置文件存在
if [ ! -f "$MACHINE_CONFIG" ]; then
    echo "ERROR: Machine config not found: $MACHINE_CONFIG"
    exit 1
fi

# Create output directory
mkdir -p "$OUTPUT_DIR"

# Step 1: Build dependencies
echo ">>> Step 1: Building kernel..."
if [ ! -f "$KERNEL_DIR/output/boot/vmlinuz-eneros" ]; then
    echo "  Building kernel..."
    (cd "$KERNEL_DIR" && ARCH=$ARCH ./build.sh)
else
    echo "  Kernel already built, skipping..."
fi

echo ">>> Step 2: Building rootfs..."
if [ ! -f "$ROOTFS_DIR/output/rootfs/bin/eneros-init" ]; then
    echo "  Building rootfs..."
    (cd "$ROOTFS_DIR" && ARCH=$ARCH ./build.sh)
else
    echo "  Rootfs already built, skipping..."
fi

echo ">>> Step 3: Building initramfs..."
if [ ! -f "$BOOT_DIR/output/initramfs.img" ]; then
    echo "  Building initramfs..."
    (cd "$BOOT_DIR" && ARCH=$ARCH ./build.sh)
else
    echo "  Initramfs already built, skipping..."
fi

# Step 4: Create image file
echo ">>> Step 4: Creating image file ($IMAGE_SIZE)..."
truncate -s "$IMAGE_SIZE" "$IMAGE"

# Step 5: Create partitions (5 分区 A/B 布局)
echo ">>> Step 5: Creating partitions..."
source "$SCRIPT_DIR/create-partitions.sh"
create_partitions "$IMAGE" "$EFI_SIZE" "$ROOT_SIZE" "$CONFIG_SIZE"

# Step 6: Loop mount the image and format partitions
echo ">>> Step 6: Mounting and formatting partitions..."
LOOP_DEVICE=$(losetup -fP --show "$IMAGE")
EFI_PART="${LOOP_DEVICE}p1"
ROOTA_PART="${LOOP_DEVICE}p2"
ROOTB_PART="${LOOP_DEVICE}p3"
DATA_PART="${LOOP_DEVICE}p4"
CONFIG_PART="${LOOP_DEVICE}p5"

# 格式化 5 个分区
format_partitions "$EFI_PART" "$ROOTA_PART" "$ROOTB_PART" "$DATA_PART" "$CONFIG_PART"

# Create mount points
MOUNT_DIR=$(mktemp -d)
EFI_MOUNT="$MOUNT_DIR/efi"
ROOTA_MOUNT="$MOUNT_DIR/roota"
ROOTB_MOUNT="$MOUNT_DIR/rootb"
DATA_MOUNT="$MOUNT_DIR/data"
CONFIG_MOUNT="$MOUNT_DIR/config"
mkdir -p "$EFI_MOUNT" "$ROOTA_MOUNT" "$ROOTB_MOUNT" "$DATA_MOUNT" "$CONFIG_MOUNT"

# Mount partitions
mount "$EFI_PART" "$EFI_MOUNT"
mount "$ROOTA_PART" "$ROOTA_MOUNT"
mount "$ROOTB_PART" "$ROOTB_MOUNT"
mount "$DATA_PART" "$DATA_MOUNT"
mount "$CONFIG_PART" "$CONFIG_MOUNT"

# Cleanup function
cleanup() {
    umount "$EFI_MOUNT" 2>/dev/null || true
    umount "$ROOTA_MOUNT" 2>/dev/null || true
    umount "$ROOTB_MOUNT" 2>/dev/null || true
    umount "$DATA_MOUNT" 2>/dev/null || true
    umount "$CONFIG_MOUNT" 2>/dev/null || true
    losetup -d "$LOOP_DEVICE" 2>/dev/null || true
    rm -rf "$MOUNT_DIR"
}
trap cleanup EXIT

# Step 7: Install rootfs to RootA (Active 槽位)
echo ">>> Step 7: Installing rootfs to RootA (Slot A)..."
if [ -f "$ROOTFS_DIR/output/eneros-rootfs-$ARCH.tar.gz" ]; then
    tar -xzf "$ROOTFS_DIR/output/eneros-rootfs-$ARCH.tar.gz" -C "$ROOTA_MOUNT"
else
    echo "ERROR: rootfs tarball not found"
    exit 1
fi

# RootB 保持空（创建空 ext4 文件系统，OTA 更新时填充）

# Step 8: Install kernel to RootA
echo ">>> Step 8: Installing kernel to RootA..."
mkdir -p "$ROOTA_MOUNT/boot"
cp "$KERNEL_DIR/output/boot/vmlinuz-eneros" "$ROOTA_MOUNT/boot/vmlinuz-eneros"
cp "$KERNEL_DIR/output/boot/System.map-eneros" "$ROOTA_MOUNT/boot/System.map-eneros" 2>/dev/null || true
cp "$KERNEL_DIR/output/boot/config-eneros" "$ROOTA_MOUNT/boot/config-eneros" 2>/dev/null || true

# Install kernel modules
if [ -d "$KERNEL_DIR/output/lib/modules" ]; then
    mkdir -p "$ROOTA_MOUNT/lib/modules"
    cp -a "$KERNEL_DIR/output/lib/modules/"* "$ROOTA_MOUNT/lib/modules/"
fi

# Step 9: Install initramfs to RootA
echo ">>> Step 9: Installing initramfs to RootA..."
cp "$BOOT_DIR/output/initramfs.img" "$ROOTA_MOUNT/boot/initramfs.img"

# Step 10: Inject machine config into RootA
echo ">>> Step 10: Injecting machine configuration..."
source "$SCRIPT_DIR/inject-config.sh"
inject_config "$ROOTA_MOUNT" "$MACHINE_CONFIG"

# Step 11: Create Config partition contents
echo ">>> Step 11: Creating Config partition contents..."
# slot-state.json — 初始状态：A=Active, B=Inactive
cat > "$CONFIG_MOUNT/slot-state.json" << 'EOF'
{
  "active_slot": "A",
  "slot_a_status": "Active",
  "slot_b_status": "Inactive",
  "boot_count_a": 0,
  "boot_count_b": 0,
  "last_boot": null,
  "last_update": null
}
EOF

# 复制机器配置到 Config 分区
cp "$MACHINE_CONFIG" "$CONFIG_MOUNT/eneros-machine.yaml"

# 创建 keys 目录（空，待生成密钥）
mkdir -p "$CONFIG_MOUNT/keys"

# Step 12: Create Data partition structure
echo ">>> Step 12: Creating Data partition structure..."
mkdir -p "$DATA_MOUNT/data/updates"

# Step 13: Install bootloader
echo ">>> Step 13: Installing bootloader..."
source "$SCRIPT_DIR/install-bootloader.sh"
install_bootloader "$EFI_MOUNT" "$ROOTA_MOUNT" "$ARCH"

# 复制 grubenv 到 EFI 分区（GRUB 环境变量块，A/B 槽位切换用）
if [ -f "$BOOT_DIR/grubenv" ]; then
    mkdir -p "$EFI_MOUNT/EFI/ENEROS"
    cp "$BOOT_DIR/grubenv" "$EFI_MOUNT/EFI/ENEROS/grubenv"
    echo "  grubenv 已复制到 EFI 分区"
fi

# Step 14: Create fstab for RootA
echo ">>> Step 14: Creating fstab..."
cat > "$ROOTA_MOUNT/etc/fstab" << EOF
# EnerOS filesystem table (Slot A)
/dev/sda2  /            ext4   defaults,noatime         0  1
/dev/sda1  /boot/efi    vfat   defaults                 0  2
/dev/sda4  /data        ext4   defaults,noatime         0  2
/dev/sda5  /config      ext4   defaults,noatime         0  2
proc       /proc        proc   defaults                 0  0
sysfs      /sys         sysfs  defaults                 0  0
devtmpfs   /dev         devtmpfs defaults               0  0
tmpfs      /run         tmpfs  defaults                 0  0
tmpfs      /tmp         tmpfs  defaults                 0  0
EOF

# Step 15: Sync and finalize
echo ">>> Step 15: Syncing filesystems..."
sync

# Calculate image size
IMAGE_SIZE_ACTUAL=$(du -sh "$IMAGE" | cut -f1)
echo "=== Image build complete ==="
echo "Image: $IMAGE"
echo "Size: $IMAGE_SIZE_ACTUAL"
echo ""
echo "Partition layout:"
echo "  /dev/sda1  EFI System (FAT32, 512MB)"
echo "  /dev/sda2  RootA (ext4, ${ROOT_SIZE}) — Active"
echo "  /dev/sda3  RootB (ext4, ${ROOT_SIZE}) — Inactive (empty, OTA target)"
echo "  /dev/sda4  Data (ext4, remaining)"
echo "  /dev/sda5  Config (ext4, ${CONFIG_SIZE})"
echo ""
echo "To test with QEMU:"
echo "  qemu-system-x86_64 -drive file=$IMAGE,format=raw -m 2G -enable-kvm"
echo "  (or without KVM: qemu-system-x86_64 -drive file=$IMAGE,format=raw -m 2G)"
