#!/bin/bash
# 复现窗：复用数据盘 + broker（REG-TELEMETRY-HEAP-REUSEBROKER 定性）——
# 窗 B（degrade）PANIC 形态复现性/时机随机性判定（峰值 vs 泄漏）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source tools/gate-env.sh
EVROOT="$ROOT/target/evidence68-vol5/reuse1"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
TPL="$ROOT/target/evidence61/template.fd"
PP_BIN="$ROOT/target/release/eneros-planner-peer"

gate_cleanup_env 1883
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave6") 2>/dev/null || true
sleep 2
mkdir -p "$EVROOT"; rm -f "$EVROOT"/*

python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 --transcript "$EVROOT/bt.jsonl" >"$EVROOT/broker.log" 2>&1 &
sleep 1
"$PP_BIN" --broker 127.0.0.1:1883 --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" --state "$EVROOT/seq.json" >"$EVROOT/pp.log" 2>&1 &
sleep 1
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/mb.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$EVROOT/i104.jsonl" --deadline 420 &
P104=$!
sleep 1
# 复用窗 A 数据盘（planner-live）
cp -f "$ROOT/target/evidence68-vol5/planner-live/eneros-data.raw" "$EVROOT/eneros-data.raw" 2>/dev/null || echo "no source disk"
cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$EVROOT" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 420 >/dev/null 2>&1
echo "qemu rc=$?"
kill $PMOD $P104 2>/dev/null || true
S="$EVROOT/qemu-serial.log"
echo "=== 判定 ==="
grep -a "PRODUCT-READY" "$S" | head -1
grep -a "PANIC" "$S" | head -2
echo "PANIC 行序号（总行数 $(wc -l < "$S")）:"
grep -an "PANIC" "$S" | head -3
grep -ac "TSDB-DISK-COMMIT " "$S"
grep -ac "EVENT-STORE append" "$S"
grep -ac "agent=OK" "$S"
