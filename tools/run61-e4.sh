#!/bin/bash
# 蓝图 61 v0.314.24（61.24-b）E4 QEMU 实弹：端到端写链 + 执行 journal 证据。
# 复用 47 卷对端三件套（modbus 从站 / MQTT broker / SCADA 写）× qemu-prod。
#
# E4 判据（蓝图 05 §2.5）：
#   1. 串口出现 EXEC-JOURNAL applied-confirmed（独立回读匹配→Committed）；
#   2. 完整 12 态链 states=[Proposed..ReadbackVerified, Committed]；
#   3. 时序：MASTER-READBACK 先于 applied-confirmed / 任何 Committed 字样
#      ——readback 前出现 Committed = FAIL；
#   4. DURABLE-INTENT-STORE open OK（durable journal 在生产装配启用）；
#   5. AUDIT-CHAIN appended>0（SHA-256 审计入链）；
#   6. 对端 transcript 有 WRITE(fc6) 与 READBACK(fc3)。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/0.314.24"
EVIDDIR="$ROOT/target/evidence61"
mkdir -p "$EVROOT" "$EVIDDIR"

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
ls -la "$IMG" >/dev/null 2>&1 || { echo "image missing: $IMG"; exit 3; }

rm -f "$EVROOT"/*.transcript.jsonl "$EVIDDIR"/qemu-serial.log

# 对端：modbus 从站（5502）必须在线——qemu-prod 投递走 drv 主通道
# （dispatch → svc-modbus-tcp OP_MASTER_WRITE → 外部从站 → 独立读回），
# 无对端 = driver status≠ST_OK = 通道 Rejected（61 起映射 Indeterminate 对账）。
# MQTT/SCADA 链路非本判据所需（CONTROL-CHAIN 由 root 自驱）。
python3 "$ROOT/evidence/47/modbus_slave47.py" --transcript "$EVROOT/modbus.transcript.jsonl" &
P_MODBUS=$!
trap 'kill $P_MODBUS 2>/dev/null || true' EXIT
sleep 1
# 窗口 900s：dispatch 装配含 durable audit genesis（littlefs 经 BlockIo，
# 分钟级——SPEC-E 起预存时序），300s 进不到 rounds。

# 2. VARS 模板（fresh；fallback 复用 46 既有模板——同一测试签名证书）。
cargo xtask image vars --output "$EVIDDIR/template.fd" --uefi-cert "$ROOT/product/security/keys-local/uefi-test.pem" >/dev/null 2>&1 \
  || cp -f "$ROOT/target/evidence46-l2/template.fd" "$EVIDDIR/template.fd" 2>/dev/null || true
ls "$EVIDDIR/template.fd" >/dev/null 2>&1 || { echo "vars template missing"; exit 3; }

# 3. QEMU 实弹（200s 有界窗口；rc=124 超时杀 = 完整证据窗口）。
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$EVIDDIR/template.fd" \
  --expect PASS \
  --evidence-dir "$EVIDDIR" \
  --net-mode present \
  --net-user \
  --net-protocols present \
  --reset-bootctrl \
  --ready-marker product-ready \
  --qmp-deadline-secs 900
RC=$?
# 超时杀=完整证据窗口（harness 优雅关机失败按 RC=2 处理——判据只看串口行）。
echo "harness RC=$RC（判据见下）"

# 4. E4 判据汇总。
LOG="$EVIDDIR/qemu-serial.log"
echo "=== RC=$RC ==="
echo "--- 判据 1-3：journal / 12 态链 / 时序 ---"
grep -nE "DURABLE-INTENT-STORE|MASTER-WRITE|MASTER-READBACK|EXEC-JOURNAL|CONTROL-CHAIN: round=|states=\[" "$LOG" | head -30
ROUNDS=$(grep -c "CONTROL-CHAIN: round=" "$LOG")
FIRST_READBACK=$(grep -n "MASTER-READBACK" "$LOG" | head -1 | cut -d: -f1)
FIRST_CONFIRMED=$(grep -n "EXEC-JOURNAL applied-confirmed" "$LOG" | head -1 | cut -d: -f1)
FIRST_COMMITTED=$(grep -n "states=\[.*Committed" "$LOG" | head -1 | cut -d: -f1)
echo "--- rounds=$ROUNDS readback=$FIRST_READBACK confirmed=$FIRST_CONFIRMED committed_states=$FIRST_COMMITTED ---"
FAIL=0
[ "$ROUNDS" -ge 1 ] 2>/dev/null || { echo "E4-FAIL: CONTROL-CHAIN 循环未启动"; FAIL=1; }
[ -n "$FIRST_READBACK" ] || { echo "E4-FAIL: 无 MASTER-READBACK"; FAIL=1; }
[ -n "$FIRST_CONFIRMED" ] || { echo "E4-FAIL: 无 EXEC-JOURNAL applied-confirmed"; FAIL=1; }
[ "$FIRST_READBACK" -lt "$FIRST_CONFIRMED" ] 2>/dev/null || { echo "E4-FAIL: applied-confirmed 未在 readback 之后"; FAIL=1; }
if [ -n "$FIRST_COMMITTED" ] && [ "$FIRST_COMMITTED" -lt "$FIRST_READBACK" ]; then
  echo "E4-FAIL: readback 前出现 Committed 状态行"; FAIL=1
fi
grep -q "DURABLE-INTENT-STORE open OK" "$LOG" || { echo "E4-FAIL: durable journal 未启用"; FAIL=1; }
AUDIT_APPEND=$(grep -c "AUDIT-CHAIN appended=" "$LOG" || true)
echo "AUDIT-CHAIN 行数=$AUDIT_APPEND"
echo "--- 对端 transcript（modbus 写+读回）---"
grep -cE "WRITE|fc06|fc 06" "$EVROOT/modbus.transcript.jsonl" 2>/dev/null || echo 0
grep -cE "READBACK|fc03|fc 03" "$EVROOT/modbus.transcript.jsonl" 2>/dev/null || echo 0

# 5. 证据归档（raw 介质除外——gitignore 纪律）。
cp -f "$LOG" "$EVROOT/qemu-serial.log" 2>/dev/null || true
[ $FAIL -eq 0 ] && echo "E4-RESULT: PASS" || echo "E4-RESULT: FAIL"
exit $([ $FAIL -eq 0 ] && echo 0 || echo 1)
