#!/usr/bin/env bash
# v0.256.0（D2）：MMS（IEC61850）客户端互操作 harness。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="${ENEROS_INTEROP_OUT:-$(cd "${SCRIPT_DIR}/../.." && pwd)/target/interop}"
mkdir -p "${OUT_DIR}"

ADDR="${ENEROS_INTEROP_MMS_ADDR:-}"
CASES='["关联", "读写", "报告订阅", "GOOSE 订阅"]'

if [ -z "${ADDR}" ]; then
    cat > "${OUT_DIR}/mms-skip.json" <<'JSON'
{
  "schema": "eneros.interop-skip.v1",
  "peer": "mms-client",
  "reason": "no real MMS client address (set ENEROS_INTEROP_MMS_ADDR)",
  "required_peer": "libiec61850 client",
  "cases": ["关联", "读写", "报告订阅", "GOOSE 订阅"],
  "acceptance": "互操作报告（14 号文 D2）"
}
JSON
    echo "[mms] skip: no real MMS client configured"
    exit 0
fi

echo "[mms] peer=${ADDR} — 真实 MMS 客户端互操作（外设就绪后执行）"
echo "[mms] cases: ${CASES}"
# 蓝图 36 WP-36.10 #1：placeholder 分支只能写 PENDING 并非零退出；
# 不允许把"harness 就绪"当作互操作 PASS。
cat > "${OUT_DIR}/mms-placeholder-evidence.json" <<'PEOF'
{
  "schema": "eneros.interop-placeholder.v1",
  "verdict": "PENDING",
  "reason": "placeholder: real peer required (no result JSON / peer identity / oracle)",
  "non_release": true
}
PEOF
echo "[mms] PENDING (placeholder: harness ready, real peer required)"
exit 1
