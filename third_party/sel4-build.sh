#!/usr/bin/env bash
# EnerOS seL4 构建（实际验证过的参数集）：内核 + libsel4 + elfloader + root task
# 平台：qemu-arm-virt / aarch64 / GICv3 / cortex-a57 / 4 核
# 证据：QEMU 启动后输出 eneros.sel4.v1 5 项 PASS（sel4-kernel-init/bootinfo-valid/
#       cap-create/ipc-loopback/root-heartbeat）
#
# WP-74.2.4（蓝图 74 卷二）：公共面收编进 sel4-build-common.sh（工具链环境/
# 依赖自检+自动 bootstrap/libcpio/converge/公共 cmake 参数单源）。本 wrapper
# 只持 C 证据路径变体：KernelRootCNodeSizeBits=13（g0 证据介质，未随 70.15
# 升 14——REG-74-A-SIZEBITS 裁决项，统一需 g0 证据重跑）+ BUILD=top + 拷 3 产物。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${SEL4_CACHE:-$HOME/.cache}"

. "$ROOT/third_party/sel4-build-common.sh"

sel4_require_cache_or_bootstrap
sel4_export_toolchain_env

BUILD="$ROOT/third_party/sel4-build/top"

build_libcpio "$BUILD"
sel4_run_converge

"$CMAKE_DIR/cmake" "${CMAKE_COMMON_ARGS[@]}" \
  -DKernelRootCNodeSizeBits=13 \
  -B "$BUILD" "$ROOT/third_party/sel4-top"
"$NINJA" -C "$BUILD"

# 产物对齐到 target/sel4（KRN-006：xtask sel4: selector 从此目录读取）
TARGET_SEL4="$ROOT/target/sel4"
mkdir -p "$TARGET_SEL4"
cp "$BUILD/elfloader/elfloader" "$TARGET_SEL4/elfloader-arm-qemu-arm-virt"
cp "$BUILD/kernel/kernel.elf" "$TARGET_SEL4/kernel.elf"
cp "$BUILD/images/eneros-root-image-arm-qemu-arm-virt" "$TARGET_SEL4/eneros-root-image-arm-qemu-arm-virt"
echo "target/sel4 产物：$(ls "$TARGET_SEL4")"
echo "镜像: $BUILD/images/eneros-root-image-arm-qemu-arm-virt"
echo "启动: qemu-system-aarch64 -machine virt,gic-version=2,secure=off -cpu cortex-a57 -smp 4 -m 1024M -nographic -kernel <镜像>（REG-74-A-R1：内核 = 平台 GICv2 驱动，gic-version=3 启动即外部 abort）"
