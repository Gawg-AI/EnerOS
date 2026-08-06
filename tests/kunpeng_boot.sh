#!/usr/bin/env bash
# v0.146.0 鲲鹏 920 真机启动脚本占位（★ 信创核心交付 — 双国产平台）
# 依据：蓝图 phase3.md §v0.146.0 §7 + spec D1 三层口径 + D7 占位脚本
# 真机验证留待硬件就绪（本机无鲲鹏 920 开发板，QEMU 不支持鲲鹏模拟）
#
# 与 phytium_boot.sh 的关系：phytium_boot.sh 是飞腾 D2000 真机启动脚本占位
# （v0.145.0），kunpeng_boot.sh 是鲲鹏 920 真机启动脚本占位（v0.146.0），
# 两者并列于 tests/ 目录，共同构成双国产平台真机验证入口，互不干扰。
# 本脚本不在 CI 中执行（CI 仅跑 host 单测 + aarch64 交叉编译，不触发真机脚本）。
#
# 用法：bash tests/kunpeng_boot.sh（在鲲鹏 920 开发板就绪后执行真机启动链）
set -euo pipefail

# ── 1. 硬件探测占位 ──
# 鲲鹏 920 硬件标识：真机环境应预置 /sys/class/kunpeng-920-marker（或类似
# udev 规则生成的硬件探测文件）。本机无鲲鹏 920 硬件时直接跳过真机验证。
# D1 第三层：真机验证留待硬件——cfg 门控早期 exit 0
if [ ! -e /sys/class/kunpeng-920-marker ]; then
    echo "[kunpeng] skip: no Kunpeng 920 hardware detected"
    echo "[kunpeng] 真机验证留待硬件就绪（蓝图 §1 ★ 信创核心交付，D1 真机路径保留口径）"
    echo "[kunpeng] 双国产平台证据：飞腾 D2000 (v0.145.0) + 鲲鹏 920 (v0.146.0)"
    exit 0
fi

# ── 2. 真机启动链（硬件就绪后扩展）──
# TODO: 硬件就绪后扩展点（蓝图 §1 ★ 信创核心——D1 第三层真机验证）
# 1. cargo build --target aarch64-unknown-none --release (含 cfg 门控鲲鹏真机寄存器写路径)
# 2. dtc -I dts -O dtb configs/kunpeng-920.dts -o build/kunpeng-920.dtb
# 3. 鲲鹏 920 开发板加载 ELF + DTB
# 4. 串口日志断言：[kernel] booting + [kunpeng] smmu ok + [kunpeng] numa ok
# 5. NUMA distances 矩阵验证 (D12 真机扩展点)
# 6. 64 核 SMP 启动验证 (D13 真机扩展点 — PSCI secondary CPU boot)
echo "[kunpeng] FAIL: hardware-ready extension not implemented"
exit 1
