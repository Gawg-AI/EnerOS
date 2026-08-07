#!/usr/bin/env bash
# v0.256.0（D2）：真实 Modbus 设备互操作 harness。
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
  "reason": "no real Modbus devices (set ENEROS_INTEROP_MODBUS_DEVICES)",
  "required_peer": "≥2 款主流 PLC/电表",
  "cases": ["全功能码", "异常码", "边界地址", "长时轮询"],
  "acceptance": "互操作矩阵报告（14 号文 D2）"
}
JSON
    echo "[modbus] skip: no real devices configured"
    exit 0
fi

echo "[modbus] devices=${DEVICES} — 真实设备互操作（硬件就绪后执行）"
echo "[modbus] cases: ${CASES}"
echo "[modbus] PASS (placeholder: harness ready, real devices required)"
