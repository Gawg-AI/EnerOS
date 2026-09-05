#!/bin/bash
# 蓝图 68 卷 68.1 WP-68.1.4：停顿级联根因探查（BP67 G8 观察项收口）。
#   相位 = BP67 G8 run#1（全服务冷启首窗、无人工干预）连跑 N 次 QEMU 标准
#   窗（720s 预算/次）；CHUNKED-SLOW 观测已在镜像内（WP-68.1.2）。
#   每窗分析：PRODUCT-READY / HEARTBEAT-KILL / SUPERVISOR / ACQ-ROUND 节奏 /
#   CHUNKED-SLOW / 服务端 ERR 面——判定 svc-telemetry 是否停顿级联。
#   证据归档 evidence/68/vol68-1/stall-probe/。
#   用法：tools/stall-probe-681.sh <run1|run2|run3>
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
RUN="${1:?usage: stall-probe-681.sh <run1|run2|run3>}"
EVROOT="$ROOT/evidence/68/vol68-1/stall-probe"
mkdir -p "$EVROOT"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing（先跑 gate 步骤 2 或 sel4-build-rust.sh）"; exit 3; }
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

gate_cleanup_env 5597 5602
gate_export_signing_key

DIR="$ROOT/target/evidence68-vol1/stall-$RUN"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
cp -f "$ROOT/evidence/62/vol62-1/normal.modbus.transcript.jsonl" \
      "$EVROOT/$RUN.modbus.transcript.jsonl"
python3 "$ROOT/evidence/61/modbus_slave61.py" \
  --transcript "$EVROOT/$RUN.modbus.transcript.jsonl" &
PPID_PEER=$!
sleep 1
if ! kill -0 $PPID_PEER 2>/dev/null; then echo "PEER-DIED"; exit 5; fi
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 720 >/dev/null 2>&1
RUNRC=$?
kill $PPID_PEER 2>/dev/null || true
cp -f "$DIR/qemu-serial.log" "$EVROOT/$RUN.qemu-serial.log" 2>/dev/null || true
echo "run rc=$RUNRC（124/2=超时杀=完整 720s 窗）"

S="$EVROOT/$RUN.qemu-serial.log"
[ -f "$S" ] || { echo "serial log missing"; exit 6; }

echo "=== $RUN 停顿判据分析 ==="
FAIL=0
if grep -q "PRODUCT-READY: true" "$S"; then echo "[✓] PRODUCT-READY: true"; else echo "[✗] PRODUCT-READY 缺失"; FAIL=1; fi
for pat in "HEARTBEAT-KILL" "SUPERVISOR"; do
  N=$(grep -c "$pat" "$S" || true)
  if [ "$N" -eq 0 ]; then echo "[✓] $pat ×0（无停顿级联）"; else echo "[✗] $pat ×$N（停顿级联复现！）"; FAIL=1; fi
done
ACQ=$(grep -c "ACQ-ROUND" "$S" || true)
echo "[i] ACQ-ROUND ×$ACQ"
if [ "$ACQ" -ge 20 ]; then echo "[✓] 采集节奏存活（基线 600s 窗 ×35 校准，≥20 合理下限）"; else echo "[✗] ACQ-ROUND 过少（遥测停顿嫌疑）"; FAIL=1; fi
SLOWN=$(grep -c "CHUNKED-SLOW" "$S" || true)
echo "[i] CHUNKED-SLOW ×$SLOWN"
grep "CHUNKED-SLOW" "$S" | head -5 || true
for pat in "CHUNKED-SLOT-FULL" "CHUNKED-DUP-SESSION" "CHUNKED-PAGE-EVICTED"; do
  N=$(grep -c "$pat" "$S" || true)
  echo "[i] $pat ×$N"
done
ERRN=$(grep -c "ERR:BadIndex" "$S" || true)
echo "[i] ERR:BadIndex ×$ERRN（迟到/交错帧观测）"
if [ $FAIL -ne 0 ]; then
  echo "=== $RUN: STALL-SUSPECTED ==="
  exit 1
fi
echo "=== $RUN: CLEAN ==="
