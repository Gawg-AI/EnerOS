#!/bin/bash
# 蓝图 70 卷 70.2（v0.332.0）QEMU 实弹窗——持久水位与复用盘恢复（WP-70.2.5）：
#   段1（seed）：fresh 数据盘 + broker 编队正常窗，产出含 TSDB
#      checkpoint/事件/快照状态的数据盘；
#   段2（reuse）：**复用段1数据盘**冷启动（WP-70.2.5 铁律：复用盘不得
#      用 fresh 替代）——TSDB mount 保数据（旧点数 >0）、事件/快照恢复、
#      决策链恢复进展；全窗零 PANIC / 零分配失败（无豁免，判据单源
#      = run70_gate_judge.py，G1–G7）。
#   判定基线：判据器自检先行（负例注入必红）；两段共用同一镜像摘要。
#   用法：SEED_DL=300 REUSE_DL=600 run70-vol2-gate.sh
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/70/70.2"
RUN="$ROOT/target/evidence70-vol2"
SEED_DL="${SEED_DL:-300}"
REUSE_DL="${REUSE_DL:-600}"
mkdir -p "$EVROOT" "$RUN/seed" "$RUN/reuse"

# ---- 段0：判据器负例自检 ----
python3 "$ROOT/tools/run70_gate_judge.py" --selftest || {
  echo "判据器自检 FAIL——门不可信"; exit 2; }

gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

launch_fleet() {  # $1=run_dir $2=deadline
  # 本轮新写：transcript 以 append 打开，run_dir 复用时不清理 = G7 恒真
  #（旧轮数据冒充本轮对端存活）——见 tools/run70_gate_judge.py G7。
  rm -f "$1/modbus.transcript.jsonl" "$1/iec104.transcript.jsonl" \
        "$1/broker-transcript.jsonl"
  python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$1/modbus.transcript.jsonl" &
  PMOD=$!
  sleep 1
  python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$1/iec104.transcript.jsonl" --deadline "$2" &
  P104=$!
  sleep 1
  python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
    --transcript "$1/broker-transcript.jsonl" >"$1/broker.log" 2>&1 &
  PBRK=$!
  sleep 1
  (ss -tln 2>/dev/null | grep -q ":1883 ") || { echo "BROKER-NOT-LISTENING"; exit 4; }
}

kill_fleet() {
  kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null || true
  sleep 3
  kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
  kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
  kill -9 $(pgrep -f "iec104_slave62") 2>/dev/null || true
  kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
  sleep 1
}

wait_ready() {  # $1=log
  for _ in $(seq 1 100); do
    grep -q "PRODUCT-READY: true" "$1" 2>/dev/null && return 0
    sleep 3
  done
  return 1
}

echo "=== 段1 seed（fresh 盘，$SEED_DL s）==="
D="$RUN/seed"; rm -f "$D/qemu-serial.log" "$D/eneros-data.raw"
launch_fleet "$D" "$SEED_DL"
cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$D" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$SEED_DL" >/dev/null 2>&1 &
QEMU_JOB=$!
wait_ready "$D/qemu-serial.log" || { echo "seed PRODUCT-READY 未达"; kill_fleet; exit 6; }
sleep "$SEED_DL"
kill_fleet
S1="$D/qemu-serial.log"
[ -f "$S1" ] || { echo "seed serial log missing"; exit 7; }
grep -q "PRODUCT-READY: true" "$S1" || { echo "seed 未 true"; exit 10; }
grep -aq "TSDB-CHECKPOINT-DUR" "$S1" || { echo "seed 窗无 checkpoint（数据盘无持久状态可复用）"; exit 12; }

echo "=== 段2 reuse（复用段1数据盘，$REUSE_DL s）==="
D2="$RUN/reuse"; rm -f "$D2/qemu-serial.log"
cp -f "$D/eneros-data.raw" "$D2/eneros-data.raw" || { echo "seed 数据盘缺失"; exit 8; }
launch_fleet "$D2" "$REUSE_DL"
cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$D2" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$REUSE_DL" >/dev/null 2>&1 &
QEMU_JOB=$!
wait_ready "$D2/qemu-serial.log" || { echo "reuse PRODUCT-READY 未达"; kill_fleet; exit 6; }
sleep "$REUSE_DL"
kill_fleet
S2="$D2/qemu-serial.log"
[ -f "$S2" ] || { echo "reuse serial log missing"; exit 7; }
cp -f "$S1" "$EVROOT/seed.qemu-serial.log"
cp -f "$S2" "$EVROOT/reuse.qemu-serial.log"
cp -f "$D/eneros-data.raw" "$EVROOT/seed.eneros-data.raw"

FAIL=0
# ---- R1：复用盘 TSDB 保数据（mount 不重建卷：旧 points >0）----------
# seed 末次 CHECKPOINT 后的 points 与 reuse 首次 OPEN points 对照。
SEED_PTS=$(grep -o "TSDB-CHECKPOINT-DUR ms=[0-9]* items=[0-9]* pending_batches=[0-9]*" "$S1" | tail -1)
OPEN_LINE=$(grep -a "TSDB-DISK-OPEN" "$S2" | head -1)
echo "[R1] seed 末次 ck：$SEED_PTS"
echo "[R1] reuse OPEN：$OPEN_LINE"
grep -aq "TSDB-DISK-OPEN" "$S2" || { echo "[✗][R1] 复用盘未走 TSDB-DISK-OPEN（mount 失败/重建）"; FAIL=1; }
POINTS=$(echo "$OPEN_LINE" | grep -o "points=[0-9]*" | grep -o "[0-9]*$")
[ -n "$POINTS" ] && [ "$POINTS" -gt 0 ] || { echo "[✗][R1] 复用盘旧点数=$POINTS（卷被重建/数据丢失）"; FAIL=1; }

# ---- R2：复用盘恢复后决策链恢复进展（判据器 G4/G5 相位判据）--------
MIN_ROUNDS="${MIN_ROUNDS:-120}"
python3 "$ROOT/tools/run70_gate_judge.py" "$S2" "$MIN_ROUNDS" \
  --window-secs "$REUSE_DL" \
  --transcripts "$D2/modbus.transcript.jsonl" "$D2/iec104.transcript.jsonl" || FAIL=1
ROUNDS=$(grep -c "CONTROL-CHAIN: round=" "$S2")
echo "[R2] reuse rounds=$ROUNDS（门限=$MIN_ROUNDS）"

# ---- R3：持久水位观测行（70.2 新词形在案）--------------------------
CK2=$(grep -ac "TSDB-CHECKPOINT-DUR" "$S2")
PEND=$(grep -o "pending_batches=[0-9]*" "$S2" | grep -o "[0-9]*$" | sort -n | tail -1)
echo "[R3] reuse checkpoint 行=$CK2 max_pending_batches=${PEND:-0}（复用盘恢复期允许显式>0）"
[ "$CK2" -ge 1 ] || { echo "[✗][R3] 复用盘窗无 checkpoint（持久水位停滞）"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then
  echo "=== VOL70.2 QEMU WINDOW: PASS ==="
else
  echo "=== VOL70.2 QEMU WINDOW: FAIL ==="
  exit 11
fi
