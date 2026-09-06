#!/bin/bash
# 蓝图 68 卷 68.5（v0.325.0）WP-68.5.4 前置探针窗（2026-09-06）：
# REG-MQTT-BROKER-HANG 现地复现排查——broker + planner-peer 在场时
# svc-mqtt 是否仍冻结（注册缺陷为 68.5 QEMU 判据关键路径，附加裁决 6）。
# 判据（探针级，非出口门）：
#   - guest 引导完成（PRODUCT-READY true）且不冻结（self_check 进入 drv loop）；
#   - broker transcript 有 guest CONNECT；
#   - planner-peer 日志 PEER-REQ/PLANNER-SOLVE/PEER-RESP 在环；
#   - guest 串口 REMOTE-BRAIN used=true + resp 带 backend=planner 词形。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source tools/gate-env.sh
EVROOT="$ROOT/target/evidence68-vol5/probe"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
TPL="$ROOT/target/evidence61/template.fd"
DL=${1:-420}

gate_cleanup_env 1883
mkdir -p "$EVROOT"; rm -f "$EVROOT"/*

python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" \
  --port 1883 --transcript "$EVROOT/broker-transcript.jsonl" \
  >"$EVROOT/broker.log" 2>&1 &
sleep 1

"$ROOT/target/release/eneros-planner-peer" \
  --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$EVROOT/seq-state.json" \
  >"$EVROOT/planner-peer.log" 2>&1 &
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi
grep -q READY "$EVROOT/planner-peer.log" || { echo "PEER-NOT-READY"; cat "$EVROOT/planner-peer.log"; exit 5; }
echo "[probe] broker + planner-peer 在场（1883）"

python3 "$ROOT/evidence/61/modbus_slave61.py" \
  --transcript "$EVROOT/modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" \
  --transcript "$EVROOT/iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1

cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$TPL" \
  --expect PASS --evidence-dir "$EVROOT" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1
RRC=$?
echo "[probe] qemu rc=$RRC（124/2=超时杀=完整窗口）"
kill $PMOD $P104 2>/dev/null || true
pkill -9 -f "mqtt_telemetry_broker.py" 2>/dev/null || true
pkill -9 -f "release/eneros-planner-peer" 2>/dev/null || true

echo "=== 探针判据扫描 ==="
S="$EVROOT/qemu-serial.log"
ok=1
grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY（引导未完成——冻结/冻结形态回放如下）"; ok=0; }
grep -q "AI-PEER-CFG loaded" "$S" && echo "[✓] AI-PEER-CFG loaded（svc-ai-worker 进 remote 路径）" || { echo "[✗] AI-PEER-CFG loaded 缺失"; ok=0; }
if grep -aq "CONNACK" "$EVROOT/broker-transcript.jsonl"; then
  echo "[✓] broker 收到 guest CONNECT（CONNACK $(grep -c CONNACK "$EVROOT/broker-transcript.jsonl") 次）"
else
  echo "[✗] broker transcript 零 CONNECT——BROKER-HANG 形态复现（CONNECT 未到达 host）"; ok=0
fi
n=$(grep -c "PEER-REQ req_id=" "$EVROOT/planner-peer.log" 2>/dev/null); n=${n:-0}
echo "[i] planner-peer 收到请求 $n 条"
if [ "$n" -ge 1 ]; then
  grep -m 2 "PLANNER-SOLVE" "$EVROOT/planner-peer.log"
  echo "[✓] planner-peer 请求在环"
else
  tail -5 "$EVROOT/planner-peer.log"
fi
grep -aq "REMOTE-BRAIN used=true" "$S" && echo "[✓] REMOTE-BRAIN used=true（System2 供给在环）" || { echo "[✗] REMOTE-BRAIN used=true 缺失"; ok=0; }
grep -m 2 "backend=planner" "$S" >/dev/null && echo "[✓] 串口出现 backend=planner 词形" || echo "[i] 串口暂无 backend=planner（样本不足或链路未通）"
grep -q "dispatch=Committed" "$S" && echo "[✓] CONTROL-CHAIN dispatch=Committed" || { echo "[✗] dispatch=Committed 缺失"; ok=0; }
n=$(grep -ac "PANIC" "$S" 2>/dev/null); n=${n:-0}
echo "[i] PANIC 行=$n"
if [ $ok -eq 1 ]; then echo "VOL68.5-PROBE: PASS"; else echo "VOL68.5-PROBE: FAIL（形态记录于本日志）"; fi
