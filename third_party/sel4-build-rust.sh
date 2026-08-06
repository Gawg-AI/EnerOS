#!/usr/bin/env bash
# ============================================================
# Track A2: Build seL4 with Rust root task (eneros-hello)
#
# 证明 Rust→seL4 FFI 端到端通：Rust ELF 经 seL4 elfloader 加载，
# 调用真实 libsel4 的 seL4_DebugPutChar 打印 banner。
#
# 前置：third_party/sel4-build.sh 已运行至少一次
#      （产出 libsel4.a + 填充 $HOME/.cache/sel4-src 等）
#
# 成功标准：QEMU 串口输出包含 "Hello from Rust on seL4!"
#
# 不替代 C root task 的产品地位（C root task 含 G3 virtio I/O）。
# 本脚本仅用于 Track A2 FFI 验证。
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

# 告诉 eneros-sel4-sys/build.rs 去哪找 libsel4.a 和 seL4 头文件
export SEL4_BUILD_DIR="$ROOT/third_party/sel4-build/top"
export SEL4_SRC="$SEL4_SRC_DIR"
export CROSS_COMPILE="aarch64-none-elf-"

echo "============================================================"
echo "Track A2: Rust root task on seL4 (FFI end-to-end proof)"
echo "============================================================"

# ---- Step 1: 构建 Rust root task ELF ----
echo ""
echo "[1/3] Building Rust root task (eneros-hello) for aarch64-unknown-none..."
cd "$ROOT"
cargo build -p eneros-hello --target aarch64-unknown-none \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem

RUST_ELF="$ROOT/target/aarch64-unknown-none/debug/eneros-hello"
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

IMAGE="$BUILD/images/eneros-root-image-arm-qemu-arm-virt"
if [[ ! -f "$IMAGE" ]]; then
  echo "ERROR: seL4 image not found at $IMAGE" >&2
  exit 1
fi
echo "  ✓ seL4 image: $IMAGE ($(stat -c%s "$IMAGE") bytes)"

# ---- Step 3: QEMU 启动 ----
echo ""
echo "[3/3] Booting in QEMU..."
echo "  期望串口输出: 'Hello from Rust on seL4!'"
echo "  退出 QEMU: Ctrl-A 然后按 X"
echo "------------------------------------------------------------"
"$CACHE/xpack-qemu-arm-9.2.4-1/bin/qemu-system-aarch64" \
  -machine virt,gic-version=3,secure=off \
  -cpu cortex-a57 -smp 4 -m 1024M \
  -nographic \
  -kernel "$IMAGE"
