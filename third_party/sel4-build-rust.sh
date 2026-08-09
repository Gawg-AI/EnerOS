#!/usr/bin/env bash
# ============================================================
# G-KERNEL（21-A）：Build seL4 with Rust root task (eneros-root-task)
#
# 产出 elfloader 镜像（RUST_ROOT_TASK=ON，root task = eneros-root-task
# release ELF），供 `cargo xtask image qemu-prod` 打包（K3.1 端到端
# 启动→spawn→IPC→故障隔离）。C2 (BOOT-007) 内核补丁由 sel4-build.sh
# 的幂等 patch 步骤（third_party/sel4-patches/）保证，本脚本假定
# $CACHE/sel4-src 已由 sel4-build.sh 准备（含补丁）。
#
# 前置：third_party/sel4-build.sh 已运行至少一次（工具链 + libsel4.a +
#       $CACHE/sel4-src 就绪）
#
# 产物：third_party/sel4-build/top-rust/elfloader/elfloader →
#       target/sel4/elfloader-arm-qemu-arm-virt（xtask 从此读取）
# ============================================================
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${SEL4_CACHE:-$HOME/.cache}"

# ---- 工具链环境（与 sel4-build.sh 一致）----
SEL4_SRC_DIR="$CACHE/sel4-src"
ELFLOADER_SRC="$CACHE/seL4_tools-master/elfloader-tool"
GCC_BIN="$CACHE/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin"
CMAKE_BIN="$CACHE/cmake-3.31.6-linux-x86_64/bin"
NINJA="$CACHE/ninja-bin/ninja"
DTC_BIN="$CACHE/dtc-deb/usr/bin"
XMLLINT_BIN="$CACHE/xmllint-bin"
export PATH="$GCC_BIN:$CMAKE_BIN:$DTC_BIN:$XMLLINT_BIN:$PATH"
export LD_LIBRARY_PATH="$CACHE/libxml2-deb/usr/lib/x86_64-linux-gnu:$CACHE/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONUSERBASE="$CACHE/pyuser"

# 告诉 eneros-sel4-sys/build.rs 去哪找 libsel4.a 和 seL4 头文件。
# 必须指向 top-rust（本脚本构建的 RUST_ROOT_TASK 内核目录）：invocation
# label（如 ARMPageTableMap）由 SEL4_BUILD_DIR 的 kernel_all_pp_prune.c
# 决定，root-task 与内核 label 不一致会在 PageTable_Map 处返回
# IllegalOperation（QEMU 实测 DBG-PTMAP err=3，21-A label 36/38 同类坑）。
export SEL4_BUILD_DIR="$ROOT/third_party/sel4-build/top-rust"
export SEL4_SRC="$SEL4_SRC_DIR"
export CROSS_COMPILE="aarch64-none-elf-"

echo "============================================================"
echo "G-KERNEL (21-A): Rust root task (eneros-root-task) on seL4"
echo "============================================================"

# ---- Step 1: 构建 Rust root task ELF（release，与 init bundle 同一产物）----
echo ""
echo "[1/3] Building Rust root task (eneros-root-task, release)..."
cd "$ROOT"
cargo build -p eneros-root-task --target aarch64-unknown-none --release \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem --locked

RUST_ELF="$ROOT/target/aarch64-unknown-none/release/eneros-root-task"
if [[ ! -f "$RUST_ELF" ]]; then
  echo "ERROR: Rust ELF not found at $RUST_ELF" >&2
  exit 1
fi
ELF_SIZE=$(stat -c%s "$RUST_ELF")
echo "  ✓ Rust ELF: $RUST_ELF ($ELF_SIZE bytes)"

# 快速校验 ELF 头（aarch64 static ELF, entry point 存在）
ELF_MAGIC=$(xxd -l 4 -p "$RUST_ELF")
if [[ "$ELF_MAGIC" != "7f454c46" ]]; then
  echo "ERROR: Not a valid ELF file (magic=$ELF_MAGIC)" >&2
  exit 1
fi
echo "  ✓ ELF magic OK"

# ---- Step 2: 构建 seL4（Rust root task 模式）----
echo ""
echo "[2/3] Building seL4 with RUST_ROOT_TASK=ON..."
BUILD="$ROOT/third_party/sel4-build/top-rust"
mkdir -p "$BUILD/elfloader"

# libcpio（elfloader 依赖，与 sel4-build.sh 一致）
(cd "$CACHE/util_libs-master/libcpio" && \
  "$GCC_BIN/aarch64-none-elf-gcc" -ffreestanding -fno-pic -fno-pie -march=armv8-a \
    -I include -c src/cpio.c -o "$BUILD/elfloader/cpio.o" && \
  ar rcs "$BUILD/elfloader/libcpio.a" "$BUILD/elfloader/cpio.o")

"$CMAKE_BIN/cmake" -G Ninja \
  -DCMAKE_MAKE_PROGRAM="$NINJA" \
  -DCMAKE_TOOLCHAIN_FILE="$SEL4_SRC_DIR/gcc.cmake" \
  -DCROSS_COMPILER_PREFIX="aarch64-none-elf-" \
  -DKernelArch=arm -DKernelSel4Arch=aarch64 \
  -DKernelPlatform=qemu-arm-virt \
  -DKernelSel4ArchAarch64=ON \
  -DPLATFORM=qemu-arm-virt \
  -DARM_CPU=cortex-a57 -DQEMU_GIC_VERSION=3 \
  -DKernelVerificationBuild=OFF -DKernelPrinting=ON \
  -DSEL4_SRC="$SEL4_SRC_DIR" \
  -DELFLOADER_SRC="$ELFLOADER_SRC" \
  -DCMAKE_MODULE_PATH="$SEL4_SRC_DIR;$CACHE/seL4_tools-master/cmake-tool/helpers;$ELFLOADER_SRC" \
  -DDTC="$DTC_BIN/dtc" \
  -DQEMU_BINARY="$CACHE/xpack-qemu-arm-9.2.4-1/bin/qemu-system-aarch64" \
  -DRUST_ROOT_TASK=ON \
  -DRUST_ROOT_ELF="$RUST_ELF" \
  -B "$BUILD" "$ROOT/third_party/sel4-top"
"$NINJA" -C "$BUILD"

# ---- Step 3: 产物对齐到 target/sel4（image 构建从此读取）----
TARGET_SEL4="$ROOT/target/sel4"
mkdir -p "$TARGET_SEL4"
cp -f "$BUILD/elfloader/elfloader" "$TARGET_SEL4/elfloader-arm-qemu-arm-virt"
echo ""
echo "[3/3] elfloader (Rust root task) → $TARGET_SEL4/elfloader-arm-qemu-arm-virt"
echo "  后续：SOURCE_DATE_EPOCH=... cargo xtask image qemu-prod"
