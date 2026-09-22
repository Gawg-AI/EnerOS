#!/usr/bin/env bash
# EnerOS seL4 构建（实际验证过的参数集）：内核 + libsel4 + elfloader + root task
# 平台：qemu-arm-virt / aarch64 / GICv3 / cortex-a57 / 4 核
# 证据：QEMU 启动后输出 eneros.sel4.v1 5 项 PASS（sel4-kernel-init/bootinfo-valid/
#       cap-create/ipc-loopback/root-heartbeat）
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${SEL4_CACHE:-$HOME/.cache}"

# 依赖（缺失时自动引导下载，21-B D3：干净容器无预置缓存可成功）
SEL4_SRC="$CACHE/sel4-src"
ELFLOADER_SRC="$CACHE/seL4_tools-master/elfloader-tool"
GCC_BIN="$CACHE/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin"
CMAKE_BIN="$CACHE/cmake-3.31.6-linux-x86_64/bin"
NINJA="$CACHE/ninja-bin/ninja"
DTC_BIN="$CACHE/dtc-deb/usr/bin"
XMLLINT_BIN="$CACHE/xmllint-bin"
# sel4-14.0.0/seL4_tools tarball 也在必需项里：补丁收敛器（converge）
# 从 tarball 还原官方原件，缺 tarball 的缓存无法自愈。
for required in "$SEL4_SRC/CMakeLists.txt" "$ELFLOADER_SRC/CMakeLists.txt" \
                "$CACHE/sel4-14.0.0.tar.gz" "$CACHE/seL4_tools.tar.gz" \
                "$GCC_BIN/aarch64-none-elf-gcc" "$CMAKE_BIN/cmake" "$NINJA" \
                "$DTC_BIN/dtc" "$XMLLINT_BIN/xmllint"; do
  if [ ! -e "$required" ]; then
    echo "[bootstrap] missing cache dependency: $required"
    echo "[bootstrap] running third_party/sel4-bootstrap.sh (SEL4_CACHE=$CACHE)"
    bash "$ROOT/third_party/sel4-bootstrap.sh"
    break
  fi
done
export PATH="$GCC_BIN:$CMAKE_BIN:$DTC_BIN:$XMLLINT_BIN:$PATH"
export LD_LIBRARY_PATH="$CACHE/libxml2-deb/usr/lib/x86_64-linux-gnu:$CACHE/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONUSERBASE="$CACHE/pyuser"

# 关键配置（历史修复要点见 docs/adr/ADR-0023）
# - KernelArch=arm（aarch64 是 KernelSel4Arch）
# - KernelSel4ArchAarch64=ON（config_choice 链路的架构标志）
# - KernelVerificationBuild=OFF + KernelPrinting=ON（验证内核禁用打印）
# - QEMU dumpdtb 生成 DTB → dtc → devices_gen.h（需 pyfdt/lxml/ply/pyelftools/libarchive）
# - libcpio 手工构建（elfloader 依赖）
BUILD="$ROOT/third_party/sel4-build/top"
# 44 Exit clean-clone 修复：cpio.o/libcpio.a 落在 $BUILD/elfloader/，先前
# 依赖历史构建目录已存在（fresh clone 失败：can't create cpio.o）；显式建目录。
mkdir -p "$BUILD/elfloader"

# 1) 构建 libcpio（util_libs）
(cd "$CACHE/util_libs-master/libcpio" && \
  "$GCC_BIN/aarch64-none-elf-gcc" -ffreestanding -fno-pic -fno-pie -march=armv8-a \
    -I include -c src/cpio.c -o "$BUILD/elfloader/cpio.o" && \
  ar rcs "$BUILD/elfloader/libcpio.a" "$BUILD/elfloader/cpio.o")

# 1.5) EnerOS 补丁面收敛（2026-09-23 审计复核施工，DEF-52-01/02 家族关闭）：
#      受管文件（boot.c/vspace.c/sys_boot.c/linker.lds）收敛到期望 sha256
#      终态——任何起始状态（fresh/旧版/部分应用 .rej/DBG-TEMP 感染/master
#      漂移）都收敛到同一字节并 fail-closed 校验。取代旧的 marker-grep
#      幂等循环（0002 无 marker 串 → fresh clone exit 1；boot.c 打上而
#      vspace.c hunk 被拒仍判「已应用」——vspace.c.rej 真实现场在案）。
bash "$ROOT/third_party/sel4-converge-patches.sh"

# 2) 顶层 CMake 配置 + 构建
"$CMAKE_BIN/cmake" -G Ninja \
  -DCMAKE_MAKE_PROGRAM="$NINJA" \
  -DCMAKE_TOOLCHAIN_FILE="$SEL4_SRC/gcc.cmake" \
  -DCROSS_COMPILER_PREFIX="aarch64-none-elf-" \
  -DKernelArch=arm -DKernelSel4Arch=aarch64 \
  -DKernelPlatform=qemu-arm-virt \
  -DKernelSel4ArchAarch64=ON \
  -DPLATFORM=qemu-arm-virt \
  -DARM_CPU=cortex-a57 -DQEMU_GIC_VERSION=3 \
  -DKernelVerificationBuild=OFF -DKernelPrinting=ON \
  -DKernelRootCNodeSizeBits=13 \
  -DSEL4_SRC="$SEL4_SRC" \
  -DELFLOADER_SRC="$ELFLOADER_SRC" \
  -DUTIL_LIBS_INCLUDE="$CACHE/util_libs-master/libcpio/include" \
  -DIMAGE_START_ADDR=0x41000000 \
  -DKernelCustomDTSOverlay="$ROOT/third_party/sel4-top/overlay-init-bundle-reserved.dts" \
  -DCMAKE_MODULE_PATH="$SEL4_SRC;$CACHE/seL4_tools-master/cmake-tool/helpers;$ELFLOADER_SRC" \
  -DDTC="$DTC_BIN/dtc" \
  -DQEMU_BINARY="$CACHE/xpack-qemu-arm-9.2.4-1/bin/qemu-system-aarch64" \
  -B "$BUILD" "$ROOT/third_party/sel4-top"
"$NINJA" -C "$BUILD"

# 4) 产物对齐到 target/sel4（KRN-006：xtask sel4: selector 从此目录读取）
TARGET_SEL4="$ROOT/target/sel4"
mkdir -p "$TARGET_SEL4"
cp "$BUILD/elfloader/elfloader" "$TARGET_SEL4/elfloader-arm-qemu-arm-virt"
cp "$BUILD/kernel/kernel.elf" "$TARGET_SEL4/kernel.elf"
cp "$BUILD/images/eneros-root-image-arm-qemu-arm-virt" "$TARGET_SEL4/eneros-root-image-arm-qemu-arm-virt"
echo "target/sel4 产物：$(ls "$TARGET_SEL4")"
echo "镜像: $BUILD/images/eneros-root-image-arm-qemu-arm-virt"
echo "启动: qemu-system-aarch64 -machine virt,gic-version=3,secure=off -cpu cortex-a57 -smp 4 -m 1024M -nographic -kernel <镜像>"
