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

echo "[ota] 负例演练（33 SE-15/18/19 硬化后判据）："
echo "[ota]   1. 伪造分区表（重算 CRC 无 HMAC）→ 必拒"
echo "[ota]   2. 不可解析版本（check_monotonic fail-closed）→ 必拒"
echo "[ota]   3. 超尺寸镜像（total_size > 分区容量）→ 必拒"

REJECT_JSON="${OUT_DIR}/ota-rejection-evidence.json"
TMP_LOG="$(mktemp /tmp/ota-reject.XXXXXX)"
# 注：不用 grep -q 直连管道（pipefail 下 grep 提前关管 → SIGPIPE → 误判 FAIL）
{
  echo "{"
  echo "  \"schema\": \"eneros.ota-rejection-evidence.v1\","
  echo "  \"cases\": ["
  # 1. 伪造分区表（pst8 判据：重算 CRC 无 HMAC 必拒）
  if cargo test -p eneros-ota-ab pst8_forged_table_without_hmac_rejected > "${TMP_LOG}" 2>&1 && grep -q "test result: ok" "${TMP_LOG}"; then
    echo "    {\"case\": \"forged-partition-table-without-hmac\", \"verdict\": \"PASS\"},"
  else
    echo "    {\"case\": \"forged-partition-table-without-hmac\", \"verdict\": \"FAIL\"},"
  fi
  # 2. 不可解析版本（check_monotonic(dev, 0.123.0) 必 Err）
  if cargo test -p eneros-ota-ab test_cry006_check_monotonic_rejects_downgrade > "${TMP_LOG}" 2>&1 && grep -q "test result: ok" "${TMP_LOG}"; then
    echo "    {\"case\": \"unparseable-version-rejected\", \"verdict\": \"PASS\"},"
  else
    echo "    {\"case\": \"unparseable-version-rejected\", \"verdict\": \"FAIL\"},"
  fi
  # 3. 超尺寸镜像（构造器校验 + 运行时防越界）
  if cargo test -p eneros-ota-ab test_oversize_total_rejected_fail_closed > "${TMP_LOG}" 2>&1 && grep -q "test result: ok" "${TMP_LOG}"; then
    echo "    {\"case\": \"oversize-image-rejected\", \"verdict\": \"PASS\"}"
  else
    echo "    {\"case\": \"oversize-image-rejected\", \"verdict\": \"FAIL\"}"
  fi
  echo "  ]"
  echo "}"
} > "${REJECT_JSON}"
rm -f "${TMP_LOG}"

echo "[ota] 三连拒绝证据：${REJECT_JSON}"
if grep -q '"verdict": "FAIL"' "${REJECT_JSON}"; then
  echo "[ota] FAIL: 存在未通过的负例判据" >&2
  exit 1
fi

# 70.7.5（实体化）：QEMU 侧安装/切槽/确认演练 = 蓝图 70.7 出口门
#（T1 安装事务：BLK_BOOT 守卫 + bootctrl 双副本 trial 写；T2 同盘重启：
# boot-manager 切 B + 运行时确认 successful + floor 推进）。
if [ "${1:-}" != "--skip-qemu" ] && [ -f tools/run70-vol7-gate.sh ]; then
    echo "[ota] QEMU 演练：run70-vol7-gate（70.7 安装/切槽/确认/回退链）"
    if bash tools/run70-vol7-gate.sh; then
        echo "[ota] 演练 PASS（VOL70.7-GATE）"
    else
        echo "[ota] 演练 FAIL（VOL70.7-GATE）——判据见 evidence/70/70.7"
        exit 1
    fi
else
    echo "[ota] QEMU 演练跳过（--skip-qemu 或 gate 脚本缺失）——预案见 plan.json"
fi

echo "[ota] 演练预案：${OUT_DIR}/ota-rehearsal-plan.json"
echo "[ota] DONE"
