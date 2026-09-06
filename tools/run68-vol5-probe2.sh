#!/bin/bash
# 短窗现场抓取：QEMU 存活期间观测 slirp 对 127.0.0.1:1883 的 socket 状态
# 与 guest SYN 是否到达 slirp（run68-vol5-probe.sh 的诊断变体，120s）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source tools/gate-env.sh
EVROOT="$ROOT/target/evidence68-vol5/probe2"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
TPL="$ROOT/target/evidence61/template.fd"

gate_cleanup_env 1883
pkill -9 -f "mqtt_telemetry_broker.py" 2>/dev/null || true
pkill -9 -f "release/eneros-planner-peer" 2>/dev/null || true
sleep 1
mkdir -p "$EVROOT"; rm -f "$EVROOT"/*

python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" \
  --port 1883 --transcript "$EVROOT/broker-transcript.jsonl" >"$EVROOT/broker.log" 2>&1 &
sleep 1
"$ROOT/target/release/eneros-planner-peer" \
  --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$EVROOT/seq-state.json" >"$EVROOT/planner-peer.log" 2>&1 &
sleep 1

python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$EVROOT/iec104.transcript.jsonl" --deadline 120 &
P104=$!
sleep 1

cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$EVROOT" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 180 >/dev/null 2>&1 &
QPID=$!
echo "[probe2] qemu launched pid=$QPID——轮询等 PRODUCT-READY（≤120s）再抓现场"
SLOG="$EVROOT/qemu-serial.log"
READY=0
for i in $(seq 1 60); do
  sleep 2
  if [ -f "$SLOG" ] && grep -aq "PRODUCT-READY: true" "$SLOG" 2>/dev/null; then READY=1; break; fi
done
echo "[probe2] READY=$READY after $((i*2))s——抓 socket 现场"
sleep 10
echo "=== QEMU 进程对 1883 的 socket ==="
(ss -tnp 2>/dev/null | grep -E ":1883" | head -10) || true
echo "=== QEMU 进程全部对外 socket（按状态） ==="
Q=$(pgrep -f "qemu-system" | head -1)
[ -n "$Q" ] && ls /proc/$Q/fd -l 2>/dev/null | grep -c socket || true
(ss -tnp 2>/dev/null | grep "pid=$Q" | head -15) || true
echo "=== broker transcript ==="
head -8 "$EVROOT/broker-transcript.jsonl" 2>/dev/null
echo "=== modbus/iec104 peer 连接证据 ==="
head -3 "$EVROOT/modbus.transcript.jsonl" 2>/dev/null
head -3 "$EVROOT/iec104.transcript.jsonl" 2>/dev/null
echo "=== mqtt 自检行 ==="
grep -a "self-check\] eneros_mqtt" "$EVROOT/qemu-serial.log" 2>/dev/null | head -4
echo "=== PRODUCT-READY ==="
grep -a "PRODUCT-READY" "$EVROOT/qemu-serial.log" 2>/dev/null | head -2
wait $QPID 2>/dev/null
kill $PMOD $P104 2>/dev/null || true
pkill -9 -f "mqtt_telemetry_broker.py" 2>/dev/null || true
pkill -9 -f "release/eneros-planner-peer" 2>/dev/null || true
echo "[probe2] done"
