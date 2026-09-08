#!/bin/bash
# 蓝图 70.1 WP-70.1.1：堆水位归因窗（短窗采样 HEAP-WM 曲线，非验收门）。
# 用法：DL=300 run70-vol1-attrib.sh
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source tools/gate-env.sh || true
DL="${DL:-300}"
D="$ROOT/target/evidence70-vol1/attrib"
mkdir -p "$D"
rm -f "$D/qemu-serial.log" "$D/eneros-data.raw"

gate_cleanup_env 5577 5582 2404 1883 || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2

python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$D/mb.jsonl" &
P1=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$D/i104.jsonl" --deadline "$DL" &
P2=$!
sleep 1
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 --transcript "$D/bt.jsonl" >"$D/broker.log" 2>&1 &
P3=$!
sleep 1

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$ROOT/target/evidence61/template.fd" \
  --expect PASS --evidence-dir "$D" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1 &
Q=$!
for _ in $(seq 1 60); do
  grep -q "PRODUCT-READY: true" "$D/qemu-serial.log" 2>/dev/null && break
  sleep 3
done
sleep "$DL"
kill $Q $P1 $P2 $P3 2>/dev/null || true
sleep 3
kill -9 $(pgrep -f qemu-system) 2>/dev/null || true

echo "=== HEAP-WM 曲线 ==="
grep -a "HEAP-WM" "$D/qemu-serial.log" || echo "无 HEAP-WM 行（探针未产出）"
echo "=== rounds / agent=OK ==="
grep -c "CONTROL-CHAIN: round=" "$D/qemu-serial.log"
grep -ac "agent=OK" "$D/qemu-serial.log"
echo "=== PANIC ==="
grep -ac "PANIC" "$D/qemu-serial.log" || true
