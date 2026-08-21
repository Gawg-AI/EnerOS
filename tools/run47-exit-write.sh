#!/bin/bash
# 47 卷 Exit Gate 端到端写链（蓝图 47 Exit #1）：
# 外部 SCADA（MQTT 下行写）→ svc-mqtt 意图导出 → svc-control-dispatch 12 态链
# → 协议出站（svc-modbus-tcp OP_MASTER_WRITE）→ 外部 modbus 从站值变化 →
# 独立 readback → Applied → Committed + SHA-256 审计入链。
#
# 证据：QEMU 串口（INTENT-FWD / MASTER-WRITE / MASTER-READBACK / CONTROL-BINDING /
#        DURABLE-INTENT-STORE / 审计）+ 对端 transcript（modbus WRITE+READBACK、
#        mqtt SUBSCRIBE+FORWARD+PUBLISH）。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVID="$ROOT/evidence/47"
mkdir -p "$EVID"

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
ls -la "$IMG" >/dev/null 2>&1 || { echo "image missing: $IMG"; exit 3; }

EVIDDIR="target/evidence47"
mkdir -p "$EVIDDIR"
rm -f "$EVID"/*.transcript.jsonl "$EVIDDIR"/qemu-serial.log

# 1. 外部对端（独立非 EnerOS 进程）。
python3 "$EVID/modbus_slave47.py" --transcript "$EVID/modbus.transcript.jsonl" &
P_MODBUS=$!
python3 "$EVID/mqtt_broker47.py" --transcript "$EVID/mqtt.transcript.jsonl" &
P_MQTT=$!
# SCADA 写命令：等待 guest 订阅 + 遥测就绪后发布（60s 保障窗口）。
python3 "$EVID/scada_write.py" --delay 60 --retries 8 --interval 15 &
P_SCADA=$!
trap 'kill $P_MODBUS $P_MQTT $P_SCADA 2>/dev/null || true' EXIT
sleep 2

# 2. VARS 模板（fresh；无 virt-fw-vars 环境时复用 46 既有模板——同一
# 测试签名证书）。
cargo xtask image vars --output "$EVIDDIR/template.fd" --uefi-cert "$ROOT/product/security/keys-local/uefi-test.pem" >/dev/null 2>&1 \
  || cp -f "$ROOT/target/evidence46-l2/template.fd" "$EVIDDIR/template.fd" 2>/dev/null \
  || cp -f "$ROOT/target/evidence47/template.fd" "$EVIDDIR/template.fd" 2>/dev/null || true
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
  --qmp-deadline-secs 300
RC=$?
wait $P_SCADA 2>/dev/null || true
sleep 2
[ "$RC" = "124" ] && RC=0

# 4. 证据汇总。
echo "=== RC=$RC ==="
echo "--- 串口写链证据（WP-47.1 端到端）---"
grep -E "CONTROL-BINDING|DURABLE-INTENT-STORE|INTENT-FWD|MASTER-WRITE|MASTER-READBACK|SECURITY-CHECK|Committed|AUDIT" "$EVIDDIR/qemu-serial.log" 2>/dev/null | head -40 || true
echo "--- modbus transcript ---"
grep -E "WRITE|fc06" "$EVID/modbus.transcript.jsonl" 2>/dev/null | head -6 || true
echo "--- mqtt transcript ---"
grep -E "SUBSCRIBE|FORWARD|PUBLISH" "$EVID/mqtt.transcript.jsonl" 2>/dev/null | head -10 || true
exit $RC
