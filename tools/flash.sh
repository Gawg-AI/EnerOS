#!/usr/bin/env bash
# ============================================================
# EnerOS / Power Native Agent OS — Flash / QEMU Boot Script
# Version: v0.3.0
# Target: WSL2 Ubuntu / native Linux (Bash)
# ============================================================
# 用途：
#   1. tools/flash.sh --qemu      — 启动 QEMU virt 验证镜像可启动
#   2. tools/flash.sh /dev/sdX    — 将镜像烧录到 SD 卡
#
# 蓝图：phase0.md §v0.3.0（真机启动 + QEMU 兜底）
# ============================================================

set -euo pipefail

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
IMAGE="${REPO_ROOT}/build/eneros-0.3.0.img"
DTB="${REPO_ROOT}/build/qemu-virt.dtb"
SEL4_KERNEL="${REPO_ROOT}/build/sel4/kernel/kernel.elf"

# Color output (only when stdout is a terminal)
if [ -t 1 ]; then
    GREEN='\033[32m'
    YELLOW='\033[33m'
    RED='\033[31m'
    NC='\033[0m'
else
    GREEN=''
    YELLOW=''
    RED=''
    NC=''
fi

# ===== 用法 =====
usage() {
    cat <<EOF
EnerOS v0.3.0 flash script

Usage:
  tools/flash.sh --qemu         Start QEMU virt to verify the image boots
  tools/flash.sh /dev/sdX       Flash build/eneros-0.3.0.img to SD card /dev/sdX

Image: ${IMAGE}
EOF
}

# ===== 模式 1：QEMU 验证 =====
run_qemu() {
    if [ ! -f "${IMAGE}" ]; then
        echo -e "${RED}✗ Image not found: ${IMAGE}${NC}" >&2
        echo -e "${YELLOW}  Run 'make build' first.${NC}" >&2
        exit 1
    fi
    echo -e "${GREEN}[QEMU]${NC} Starting QEMU virt with ${IMAGE}"
    # 参数参考 Makefile run 目标（machine=virt, cpu=cortex-a57, 128M, smp 2）
    qemu-system-aarch64 \
        -machine virt \
        -cpu cortex-a57 \
        -m 128M \
        -smp 2 \
        -kernel "${SEL4_KERNEL}" \
        -dtb "${DTB}" \
        -append "console=ttyAMA0,115200" \
        -serial mon:stdio \
        -nographic
}

# ===== 模式 2：SD 卡烧录 =====
flash_sd() {
    local dev="$1"
    # 设备参数校验：必须以 /dev/ 开头
    if [[ ! "$dev" =~ ^/dev/ ]]; then
        echo -e "${RED}✗ Invalid device: ${dev}${NC}" >&2
        echo -e "${YELLOW}  Device must start with /dev/ (e.g. /dev/sdb)${NC}" >&2
        exit 1
    fi
    # 镜像必须存在
    if [ ! -f "${IMAGE}" ]; then
        echo -e "${RED}✗ Image not found: ${IMAGE}${NC}" >&2
        echo -e "${YELLOW}  Run 'make build' first.${NC}" >&2
        exit 1
    fi
    # 系统盘保护：sda / nvme0n0 / mmcblk0 视为高风险，拒绝烧录
    case "$dev" in
        /dev/sda|/dev/nvme0n0|/dev/mmcblk0)
            echo -e "${RED}⚠ WARNING: ${dev} looks like a system disk!${NC}" >&2
            echo -e "${YELLOW}  Refusing to flash to a likely system disk.${NC}" >&2
            exit 1
            ;;
    esac
    # 二次确认：要求用户重新输入设备名
    echo -e "${YELLOW}⚠ About to write ${IMAGE} to ${dev}${NC}"
    echo -e "${YELLOW}  ALL DATA ON ${dev} WILL BE LOST.${NC}"
    read -r -p "Type the device name to confirm (${dev}): " confirm
    if [ "$confirm" != "$dev" ]; then
        echo -e "${RED}✗ Confirmation mismatch, aborting.${NC}" >&2
        exit 1
    fi
    # 烧录（dd + sync 确保落盘）
    echo -e "${GREEN}[FLASH]${NC} Writing ${IMAGE} -> ${dev} ..."
    sudo dd if="${IMAGE}" of="${dev}" bs=4M conv=fsync status=progress
    sync
    echo -e "${GREEN}✓ Flash complete. Eject ${dev} and insert into target board.${NC}"
}

# ===== 入口 =====
case "${1:-}" in
    --qemu)
        run_qemu
        ;;
    ""|-h|--help)
        usage
        exit 0
        ;;
    *)
        flash_sd "$1"
        ;;
esac
