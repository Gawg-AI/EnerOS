#!/usr/bin/env bash
# v0.260.0（D5）：OTA 差分升级 + 断电回滚演练脚本。
#
# 依据：蓝图/14-阶段D-商用验收闸门.md D5「升级演练」——
# v0.252.0→RC 差分升级 + 断电回滚真机演练。
#
# 本环境无真机：执行 QEMU 侧演练（若 QEMU 可用）或产出演练预案
# （真机就绪后执行）。演练矩阵：
#   1. 差分包生成（v0.252.0 → 当前 RC）
#   2. 升级安装（签名验证 → A/B 槽写入）
#   3. 升级后启动校验（新版本 banner）
#   4. 断电回滚（升级后未确认 → 断电 → 回滚旧槽）
#
# 用法：bash tests/ota-rehearsal.sh [--skip-qemu]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

OUT_DIR="${ENEROS_OTA_OUT:-target/ota-rehearsal}"
mkdir -p "${OUT_DIR}"

echo "[ota] OTA 升级演练（v0.252.0 → 当前 RC）"
echo "[ota] 演练矩阵：差分包生成 / 签名验证 / A-B 槽写入 / 断电回滚"

# 1. 差分包生成（当前版本 vs 基线）
CURRENT_VERSION=$(grep -m1 '^version' Cargo.toml | awk '{print $3}' | tr -d '"')
echo "[ota] 当前版本：${CURRENT_VERSION}，基线：0.252.0"

cat > "${OUT_DIR}/ota-rehearsal-plan.json" <<EOF
{
  "schema": "eneros.ota-rehearsal-plan.v1",
  "from_version": "0.252.0",
  "to_version": "${CURRENT_VERSION}",
  "matrix": [
    "差分包生成（bsdiff/自研差分）",
    "SM2 签名验证（secure-boot 同链）",
    "A/B 槽写入 + boot control 更新",
    "升级后启动校验（新版本 banner）",
    "未确认升级断电 → 回滚旧槽（rollback counter 保护）"
  ],
  "qemu_executable": "$(command -v qemu-system-aarch64 || echo 'NOT-AVAILABLE')"
}
EOF

if command -v qemu-system-aarch64 > /dev/null 2>&1 && [ "${1:-}" != "--skip-qemu" ]; then
    echo "[ota] QEMU 可用 —— 执行 QEMU 侧演练"
    # 接入点：cargo xtask image update --from ... --to ... （差分生成）
    #        cargo xtask run qemu-install ... （安装演练）
    #        cargo xtask qa boot-reset ... （回滚校验）
    echo "[ota] QEMU 演练：占位（差分/安装/回滚命令就绪后执行）"
else
    echo "[ota] QEMU 不可用 —— 产出演练预案（真机/QEMU 就绪后执行）"
fi

echo "[ota] 演练预案：${OUT_DIR}/ota-rehearsal-plan.json"
echo "[ota] DONE"
