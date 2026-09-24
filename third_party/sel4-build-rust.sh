#!/usr/bin/env bash
# ============================================================
# G-KERNEL（21-A）：Build seL4 with Rust root task (eneros-root-task)
#
# 产出 elfloader 镜像（RUST_ROOT_TASK=ON，root task = eneros-root-task
# release ELF），供 `cargo xtask image qemu-prod` 打包（K3.1 端到端
# 启动→spawn→IPC→故障隔离）。C2 (BOOT-007) 内核补丁面由
# sel4-converge-patches.sh 收敛器保证（本脚本内自调用，独立重链安全）。
#
# 前置：third_party/sel4-build.sh 已运行至少一次（工具链 + libsel4.a +
#       $CACHE/sel4-src 就绪）
#
# 产物：third_party/sel4-build/top-rust/elfloader/elfloader →
#       target/sel4/elfloader-arm-qemu-arm-virt（xtask 从此读取）
#
# WP-74.2.4（蓝图 74 卷二）：公共面收编进 sel4-build-common.sh；本 wrapper
# 持 rust 路径变体：cargo root-task 构建、SEL4_BUILD_DIR/SEL4_SRC/CROSS_COMPILE
# 导出（留在 wrapper，避免污染 C 路径环境）、KernelRootCNodeSizeBits=14
# （70.15 实弹槽位耗尽 13→14——REG-74-A-SIZEBITS 裁决项）+ rust 专属 4 参
# + BUILD=top-rust + 拷 1 产物。
# ============================================================
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="${SEL4_CACHE:-$HOME/.cache}"

. "$ROOT/third_party/sel4-build-common.sh"

sel4_require_cache_or_bootstrap
sel4_export_toolchain_env

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
# 蓝图 40 收尾：sel4-eipc feature——root 驱动控制链（INGEST/TICK）的
# Sel4IpcClient/ChunkedClient（EIPC 端点 cap 已由 spawn 授予）。
# SPEC-E：ROOT_TASK_FEATURES 可参（缺省 sel4-eipc）——qemu-test 介质
# 经 ROOT_TASK_FEATURES="sel4-eipc,qemu-test-synth" 启用合成遥测注入
# （45.5 门控设计意图；release 装配不得设置——closure gate 强制 synth
# 不进 release 闭包）。
cargo build -p eneros-root-task --target aarch64-unknown-none --release \
  -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem --locked \
  --features "${ROOT_TASK_FEATURES:-sel4-eipc}"

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

sel4_run_converge
build_libcpio "$BUILD"

# 41（WP 装配）：root CSpace 12→13 bits（4096→8192 槽）——34 个 EL0
# 服务（40 控制链 5 + 41 业务/运维 8 + 协议/驱动/demo）spawn 期
# NoCapSlot 挂起（QEMU 实测 root_slot=4089 时耗尽）；8192 槽余量
# > 2× 当前 34 服务峰值，为后续服务预留（跨版本变更登记见
# evidence/41/CROSS_VERSION-41.md CV-41-1）。
# 2026-09-14（70.15 实弹）：13 → 14（8192 → 16384 槽）。长窗映像（新增停顿/
# 故障注入面 + coverage/语义点）使 root 槽位耗到 8191/8192 的边缘，一处页界
# 移动即 `spawn FAILED: NoCapSlot — 挂起`（svc-mgmt 段，实测）。同 41-CV-41-1
# 先例（12→13）。root cnode 对象 = 16384×16B = 256 KiB untyped，1 GiB 内存
# 充裕；root-task 侧 `ROOT_CNODE_SLOTS` 已同步 =1<<14。
# 33 WP-33.5（中断计数转真）：-DKernelArmExportPTMRUser=ON —— 允许
# EL0 写 CNTP_CTL_EL0/CNTP_TVAL_EL0（seL4 aarch64 用户态在 EL0；
# root-task 使能物理定时器需 EXPORT_PTMR_USER=EL0PTEN——缺省时
# msr cntp_ctl_el0 触发 user exception EC=0x18，QEMU 实测）。与 PCNT
# 同属开发/证据配置（存储通道警告接受：QEMU 证据用途）。
"$CMAKE_DIR/cmake" "${CMAKE_COMMON_ARGS[@]}" \
  -DKernelRootCNodeSizeBits=14 \
  -DRUST_ROOT_TASK=ON \
  -DRUST_ROOT_ELF="$RUST_ELF" \
  -DKernelArmExportPCNTUser=ON \
  -DKernelArmExportPTMRUser=ON \
  -B "$BUILD" "$ROOT/third_party/sel4-top"
# 23 G-I 总出口修复（KernelArmExportPCNTUser=ON）：允许 EL0 读
# CNTPCT_EL0/CNTFRQ_EL0（root-task time_init 的 Arm64Timer 依赖）。seL4
# 默认关闭（EXPORT_PCNT_USER disabled）→ `mrs cntpct_el0` 触发 cap fault
# （QEMU 实测）。
"$NINJA" -C "$BUILD"

# ---- Step 3: 产物对齐到 target/sel4（image 构建从此读取）----
TARGET_SEL4="$ROOT/target/sel4"
mkdir -p "$TARGET_SEL4"
cp -f "$BUILD/elfloader/elfloader" "$TARGET_SEL4/elfloader-arm-qemu-arm-virt"
echo ""
echo "[3/3] elfloader (Rust root task) → $TARGET_SEL4/elfloader-arm-qemu-arm-virt"
echo "  后续：SOURCE_DATE_EPOCH=... cargo xtask image qemu-prod"
