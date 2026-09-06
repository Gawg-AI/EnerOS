#!/bin/bash
# 68.5 窗 B 单窗复验（降级链；完整 scan_window_degrade 判据）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source tools/gate-env.sh
RUN="$ROOT/target/evidence68-vol5"
PP_BIN="$ROOT/target/release/eneros-planner-peer"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
TPL="$ROOT/target/evidence61/template.fd"

gate_cleanup_env 1883 5597 5602 2404
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave6") 2>/dev/null || true
sleep 2
mkdir -p "$RUN/degrade"; rm -f "$RUN/degrade/qemu-serial.log" "$RUN/degrade/eneros-data.raw"

python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 --transcript "$RUN/degrade.broker-transcript.jsonl" >"$RUN/degrade.broker.log" 2>&1 &
sleep 1
"$PP_BIN" --broker 127.0.0.1:1883 --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" --state "$RUN/degrade.seq-state.json" >"$RUN/degrade.planner-peer.log" 2>&1 &
sleep 1
ss -tln | grep -q ":1883 " || { echo "BROKER-NOT-LISTENING"; exit 4; }

(
  KILLED=0
  for i in $(seq 1 240); do
    sleep 2
    if [ $KILLED -eq 0 ]; then
      N=$(grep -c "PEER-REQ" "$RUN/degrade.planner-peer.log" 2>/dev/null)
      N=${N:-0}
      if [ "$N" -ge 4 ] 2>/dev/null; then
        echo "[gate] 杀 planner-peer 制造降级（PEER-REQ=$N）"
        pkill -9 -f "release/eneros-planner-peer" 2>/dev/null || true
        KILLED=1
      fi
    else
      sleep 200
      echo "[gate] 重启 planner-peer（失联 200s ≥ 3 次超时 → unhealthy 迁移）"
      "$PP_BIN" --broker 127.0.0.1:1883 \
        --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
        --state "$RUN/degrade.seq-state.json" >>"$RUN/degrade.planner-peer.log" 2>&1 &
      break
    fi
  done
) &
WATCH=$!

cp -f "$ROOT/evidence/62/vol62-3/normal.modbus.transcript.jsonl" "$RUN/degrade.mb.jsonl"
cp -f "$ROOT/evidence/62/vol62-3/normal.iec104.transcript.jsonl" "$RUN/degrade.i104.jsonl"
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$RUN/degrade.mb.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$RUN/degrade.i104.jsonl" --deadline 480 &
P104=$!
sleep 1
cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$RUN/degrade" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 480 >/dev/null 2>&1
echo "window degrade rc=$?"
kill $WATCH $PMOD $P104 2>/dev/null || true
cp -f "$RUN/degrade/qemu-serial.log" "$ROOT/evidence/68/vol68-5/degrade.qemu-serial.log" 2>/dev/null || true
pkill -9 -f "mqtt_telemetry_broker.py" 2>/dev/null || true
pkill -9 -f "release/eneros-planner-peer" 2>/dev/null || true

S="$RUN/degrade/qemu-serial.log"
echo "=== 窗 B 判据 ==="
for pat in "event=peer-unhealthy" "LEASE-ARMED level=D3" "LEASE-REVOKED reason=peer-accepted" "event=peer-recovered"; do
  if grep -aq "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; fi
done
grep -aq "LEASE-ARM-FAIL" "$S" && echo "[✗] LEASE-ARM-FAIL" || echo "[✓] LEASE-ARM-FAIL=0"
echo "healthy=false 行数: $(grep -ac 'healthy=false' "$S")"
grep -ac "REMOTE-BRAIN used=true" "$S"
