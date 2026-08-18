#!/usr/bin/env bash
# v0.256.0（D2）：真实 Modbus 设备互操作 harness。
#
# 21-B B1（2026-08-09）：对端已从"无"升级为 QEMU loopback——生产
# transport（SmoltcpTcpTransport / Rs485RtuTransport）已在 loopback 集成
# 测试中完成请求-响应闭环（tests/loopback.rs、pl011_loopback.rs）。
# 本 harness 保留给真机（真设备到位仅换对端地址）。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="${ENEROS_INTEROP_OUT:-$(cd "${SCRIPT_DIR}/../.." && pwd)/target/interop}"
mkdir -p "${OUT_DIR}"

DEVICES="${ENEROS_INTEROP_MODBUS_DEVICES:-}"
CASES='["全功能码", "异常码", "边界地址", "长时轮询"]'

if [ -z "${DEVICES}" ]; then
    cat > "${OUT_DIR}/modbus-skip.json" <<'JSON'
{
  "schema": "eneros.interop-skip.v1",
  "peer": "modbus-device",
  "reason": "no real Modbus devices (set ENEROS_INTEROP_MODBUS_DEVICES); QEMU loopback 对通已完成（21-B B1）",
  "required_peer": "≥2 款主流 PLC/电表（QEMU loopback 已闭环）",
  "cases": ["全功能码", "异常码", "边界地址", "长时轮询"],
  "acceptance": "互操作矩阵报告（14 号文 D2）"
}
JSON
    echo "[modbus] skip: no real devices configured"
    exit 0
fi

echo "[modbus] devices=${DEVICES} — 真实设备互操作（硬件就绪后执行）"
echo "[modbus] cases: ${CASES}"
# 蓝图 36 WP-36.10 #1：placeholder 分支只能写 PENDING 并非零退出；
# 不允许把"harness 就绪"当作互操作 PASS。
cat > "${OUT_DIR}/modbus-placeholder-evidence.json" <<'PEOF'
{
  "schema": "eneros.interop-placeholder.v1",
  "verdict": "PENDING",
  "reason": "placeholder: real peer required (no result JSON / peer identity / oracle)",
  "non_release": true
}
PEOF
echo "[modbus] PENDING (placeholder: harness ready, real peer required)"
exit 1
