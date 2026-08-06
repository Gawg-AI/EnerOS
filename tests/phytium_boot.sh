#!/usr/bin/env bash
# v0.145.0 飞腾 D2000 真机启动脚本占位（★ 信创核心交付）
# 蓝图 §1 ★ ADR-0001 BSP 收敛——飞腾为生产平台，真机路径保留
# 本机无飞腾 D2000 硬件 → cfg 门控早期 exit 0 + [phytium] skip 标注
# 硬件就绪后扩展：交叉编译 bootloader + kernel → dtb 加载 → 飞腾开发板串口启动
#   → 断言 "[kernel] EnerOS kernel booting" + "[phytium] smmu ok" + "[phytium] crypto ok"
#
# 与 boot_test.sh 的关系：boot_test.sh 是 QEMU virt 启动脚本（v0.129.0 起 17 个版本
# 扩展），phytium_boot.sh 是飞腾真机启动脚本占位，两者并列于 tests/ 目录，互不干扰。
# 本脚本不在 CI 中执行（CI 仅跑 host 单测 + aarch64 交叉编译，不触发真机脚本）。
#
# 用法：bash tests/phytium_boot.sh（在飞腾 D2000 开发板就绪后执行真机启动链）
set -euo pipefail

# ── 1. 硬件探测占位 ──
# 飞腾 D2000 硬件标识：真机环境应预置 /sys/class/phytium-d2000-marker（或类似
# udev 规则生成的硬件探测文件）。本机无飞腾 D2000 硬件时直接跳过真机验证。
if [ ! -e /sys/class/phytium-d2000-marker ]; then
    echo "[phytium] skip: no Phytium D2000 hardware detected"
    echo "[phytium] 真机验证留待硬件就绪（蓝图 §1 ★ 信创核心交付，D1 真机路径保留口径）"
    exit 0
fi

# ── 2. 真机启动链（硬件就绪后扩展）──
# TODO: cargo build -p eneros-bootloader -p eneros-microkernel --release --target aarch64-unknown-none
# TODO: dtc -I dts -O dtb -o configs/phytium-d2000.dtb configs/phytium-d2000.dts
# TODO: 飞腾开发板加载 bootloader.bin + kernel.bin + phytium-d2000.dtb
# TODO: 串口日志断言 "[kernel] EnerOS kernel booting" + "[phytium] smmu ok" + "[phytium] crypto ok"
# TODO: SMMUv3 CR0 使能回读 + crypto SM4/SM2 CPUID 检测 + DTB 节点 reg 实际值调试
echo "[phytium] FAIL: hardware-ready extension not implemented"
exit 1
