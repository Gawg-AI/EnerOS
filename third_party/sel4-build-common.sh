#!/usr/bin/env bash
# ============================================================
# EnerOS seL4 构建公共面（WP-74.2.4，蓝图 74 卷二 / R15-A）
#
# 被 sel4-build.sh / sel4-build-rust.sh source（非独立入口——本文件
# 只定义变量与函数，末尾无任何执行语句）。单源范围：
#   - ROOT/CACHE 解析 + 工具链 8 路径 + 3 exports（目录版本名单点定义，
#     bootstrap fetch 侧自有钉版结构，跨侧不单源——观察项，见蓝图 §2.4）
#   - 依赖自检 + 自动 bootstrap 环（原 build.sh 独有；rust 路径同受益
#     ——冷缓存此前在 cmake 步骤才以裸错误失败）
#   - build_libcpio（原两脚本逐字姊妹拷贝的 4 行子壳）
#   - converge 调用
#   - CMAKE_COMMON_ARGS（两路径 18+ 公共参数；KernelRootCNodeSizeBits
#     与 rust 4 参为路径变体，留各自 wrapper）
#
# 有意保留的分叉（非本公共面职责）：KernelRootCNodeSizeBits 13（C 证据
# 路径，g0 证据介质）vs 14（rust 产品主线，70.15 实弹槽位耗尽）——统一
# 需 g0 证据重跑，登记 REG-74-A-SIZEBITS 裁决项。
# ============================================================

# ---- ROOT/CACHE 解析（蓝图 §2.4 单源清单项）----
# BASH_SOURCE[0] = 本文件被 source 时的自身路径 → dirname/.. = 仓根；
# wrapper/沙箱可预置 ROOT/CACHE 传入（:- 缺省不覆盖）。
ROOT="${ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
CACHE="${CACHE:-${SEL4_CACHE:-$HOME/.cache}}"

# ---- 工具链路径（版本目录名单点定义）----
SEL4_SRC_DIR="$CACHE/sel4-src"
ELFLOADER_SRC="$CACHE/seL4_tools-master/elfloader-tool"
GCC_DIR="$CACHE/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin"
CMAKE_DIR="$CACHE/cmake-3.31.6-linux-x86_64/bin"
NINJA="$CACHE/ninja-bin/ninja"
DTC_DIR="$CACHE/dtc-deb/usr/bin"
XMLLINT_DIR="$CACHE/xmllint-bin"
QEMU_BIN="$CACHE/xpack-qemu-arm-9.2.4-1/bin/qemu-system-aarch64"

# ---- 依赖自检 + 自动 bootstrap（原 sel4-build.sh :20-35 迁入）----
# sel4-14.0.0/seL4_tools tarball 也在必需项里：补丁收敛器（converge）
# 从 tarball 还原官方原件，缺 tarball 的缓存无法自愈。
sel4_require_cache_or_bootstrap() {
  for required in "$SEL4_SRC_DIR/CMakeLists.txt" "$ELFLOADER_SRC/CMakeLists.txt" \
                  "$CACHE/sel4-14.0.0.tar.gz" "$CACHE/seL4_tools.tar.gz" \
                  "$GCC_DIR/aarch64-none-elf-gcc" "$CMAKE_DIR/cmake" "$NINJA" \
                  "$DTC_DIR/dtc" "$XMLLINT_DIR/xmllint"; do
    if [ ! -e "$required" ]; then
      echo "[bootstrap] missing cache dependency: $required"
      echo "[bootstrap] running third_party/sel4-bootstrap.sh (SEL4_CACHE=$CACHE)"
      SEL4_CACHE="$CACHE" bash "$ROOT/third_party/sel4-bootstrap.sh"
      break
    fi
  done
}

# ---- exports（C/rust 两路径同集；rust 额外的 SEL4_BUILD_DIR/SEL4_SRC/
#      CROSS_COMPILE 导出留在 build-rust.sh wrapper，避免污染 C 路径环境）----
sel4_export_toolchain_env() {
  export PATH="$GCC_DIR:$CMAKE_DIR:$DTC_DIR:$XMLLINT_DIR:$PATH"
  export LD_LIBRARY_PATH="$CACHE/libxml2-deb/usr/lib/x86_64-linux-gnu:$CACHE/libicu74-deb/usr/lib/x86_64-linux-gnu"
  export PYTHONUSERBASE="$CACHE/pyuser"
}

# ---- libcpio（elfloader 依赖；44 Exit clean-clone 修复：显式建目录）----
build_libcpio() { # $1 = BUILD 目录
  local build_dir="$1"
  mkdir -p "$build_dir/elfloader"
  (cd "$CACHE/util_libs-master/libcpio" && \
    "$GCC_DIR/aarch64-none-elf-gcc" -ffreestanding -fno-pic -fno-pie -march=armv8-a \
      -I include -c src/cpio.c -o "$build_dir/elfloader/cpio.o" && \
    ar rcs "$build_dir/elfloader/libcpio.a" "$build_dir/elfloader/cpio.o")
}

# ---- 补丁面收敛（2026-09-23 审计复核施工，DEF-52-01/02 家族关闭）----
# 复查轮修复：显式传递 SEL4_CACHE——converge 自行解析
# SEL4_CACHE:-$HOME/.cache，不传时自定义 CACHE 的调用会构建 /custom 而
# 收敛 ~/.cache（两树皆有完整 tarball 时静默不收敛）。
sel4_run_converge() {
  SEL4_CACHE="$CACHE" bash "$ROOT/third_party/sel4-converge-patches.sh"
}

# ---- 两路径公共 cmake 参数（变体参数由 wrapper 追加在数组之后）----
# 关键配置（历史修复要点见 docs/adr/ADR-0023）：
# - KernelArch=arm（aarch64 是 KernelSel4Arch）
# - KernelSel4ArchAarch64=ON（config_choice 链路的架构标志）
# - KernelVerificationBuild=OFF + KernelPrinting=ON（验证内核禁用打印）
# - QEMU dumpdtb 生成 DTB → dtc → devices_gen.h（需 pyfdt/lxml/ply/pyelftools/libarchive）
# - KernelCustomDTSOverlay：保留窗（BOOT-007 契约，对拍锁见 boot-protocol
#   tests/window_contract_lock.rs）
CMAKE_COMMON_ARGS=(
  -G Ninja
  -DCMAKE_MAKE_PROGRAM="$NINJA"
  -DCMAKE_TOOLCHAIN_FILE="$SEL4_SRC_DIR/gcc.cmake"
  -DCROSS_COMPILER_PREFIX="aarch64-none-elf-"
  -DKernelArch=arm -DKernelSel4Arch=aarch64
  -DKernelPlatform=qemu-arm-virt
  -DKernelSel4ArchAarch64=ON
  -DPLATFORM=qemu-arm-virt
  # QEMU_GIC_VERSION=3 在 seL4 14.0.0 为惰性变量——驱动选择由平台硬编码
  # gic_v2 决定；唯一消费者 = seL4_tools simulation.cmake 的
  # GenerateSimulateScript（本构建形态零调用方 = 死代码；74 复查轮订正：
  # 旧注「复查轮全树 grep 实证零消费者」失实，该函数在册但未接线）。
  # 保留作未来 seL4 版本消费预留（勿据此推断内核 GIC 版本）。
  -DARM_CPU=cortex-a57 -DQEMU_GIC_VERSION=3
  -DKernelVerificationBuild=OFF -DKernelPrinting=ON
  -DSEL4_SRC="$SEL4_SRC_DIR"
  -DELFLOADER_SRC="$ELFLOADER_SRC"
  -DUTIL_LIBS_INCLUDE="$CACHE/util_libs-master/libcpio/include"
  -DIMAGE_START_ADDR=0x41000000
  -DKernelCustomDTSOverlay="$ROOT/third_party/sel4-top/overlay-init-bundle-reserved.dts"
  -DCMAKE_MODULE_PATH="$SEL4_SRC_DIR;$CACHE/seL4_tools-master/cmake-tool/helpers;$ELFLOADER_SRC"
  -DDTC="$DTC_DIR/dtc"
  -DQEMU_BINARY="$QEMU_BIN"
)
