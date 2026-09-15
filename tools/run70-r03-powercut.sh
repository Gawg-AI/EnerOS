#!/bin/bash
# 蓝图 70 R03（CORE-70-R03，owner 70.2）掉电窗：checkpoint 持久状态在
# **脏断电**（kill -9 QEMU = 瞬时失电，无卸载无落盘收尾）后的恢复合同。
#
#   段1  seed：fresh 数据盘正常窗（≥3 次 TSDB checkpoint ⇒ 盘上有持久状态）；
#   段2  断电循环 ×2：同一数据盘（复制/直继，永不 fresh 替代——WP-70.2.5
#        铁律）先后两次带电运行中途 kill -9（cycle1 = checkpoint 完成后
#        短延时切；cycle2 = ready 后固定窗中段切——两相位覆盖）；
#   段3  终窗恢复判据：复用盘 mount 保数据（points>0）、逐次脏重启
#        PRODUCT-READY、零 PANIC/零分配失败（全日志）、控制链恢复进展
#        （CONTROL-CHAIN rounds/ingest）、checkpoint 恢复。
#
# 慢盘/满盘/flush-write 失败子面：窗口级注入面未建（如实）——由驱动单元
# 红测承载（RamMmio::stall_next/fail_next、保留背压红测；登记
# REG-70.15-R03-WINDOW-SCOPE）。本窗判据 = 掉电恢复主链。
#
# 用法：SEED_DL=300 CUT_WAIT=300 FINAL_DL=300 tools/run70-r03-powercut.sh
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/70/70.2/powercut"
RUN="$ROOT/target/evidence70-r03"
SEED_DL="${SEED_DL:-300}"
FINAL_DL="${FINAL_DL:-300}"
mkdir -p "$EVROOT" "$RUN"

FAIL=0

# ---- 段0：自检 + 清场 -------------------------------------------------------
python3 "$ROOT/tools/run70_gate_judge.py" --selftest >/dev/null || {
  echo "判据器自检 FAIL"; exit 2; }
gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2

# ---- 段1：构建（qemu-prod；显式重链接保证与提交闭包一致）--------------------
echo "=== 段1：重链接 + image raw qemu-prod ==="
gate_relink_cached "$RUN/sel4.log" || {
  echo "SEL4-RELINK FAIL"; tail -20 "$RUN/sel4.log"; exit 3; }
gate_cleanup_env
gate_export_signing_key
gate_compile_config "$RUN/compile.log" qemu-prod
gate_image_raw_cached qemu-prod "$RUN/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

launch_fleet() {  # $1=run_dir $2=deadline
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

# 断电切电（无收尾）：先 SIGKILL QEMU 本体（数据盘即刻冻结），再清编队。
power_cut() {
  kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
  sleep 2
  kill $PMOD $P104 $PBRK 2>/dev/null || true
  sleep 2
  kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
  kill -9 $(pgrep -f "iec104_slave62") 2>/dev/null || true
  kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
  sleep 1
}

wait_ready_and_checkpoints() {  # $1=log $2=需要的 checkpoint 数 $3=最长等待 s
  local got=0
  for _ in $(seq 1 $(("$3" / 3))); do
    grep -q "PRODUCT-READY: true" "$1" 2>/dev/null || { sleep 3; continue; }
    got=$(grep -ac "TSDB-CHECKPOINT-DUR" "$1" 2>/dev/null || true)
    [ "$got" -ge "$2" ] && return 0
    sleep 3
  done
  return 1
}

run_window() {  # $1=dir $2=deadline $3=data-disk
  mkdir -p "$1"
  rm -f "$1/qemu-serial.log"
  launch_fleet "$1" "$2"
  cargo xtask run qemu-prod \
    --image "$IMG" --vars-template "$TPL" \
    --expect PASS --evidence-dir "$1" \
    --data-disk "$3" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$2" >/dev/null 2>&1 &
  QEMU_JOB=$!
}

# ---- 段2 seed：fresh 盘，攒 ≥3 次 checkpoint --------------------------------
echo "=== 段2 seed（fresh 盘，${SEED_DL}s，≥3 checkpoint）==="
D="$RUN/seed"; mkdir -p "$D"; rm -f "$D/eneros-data.raw"; truncate -s 16M "$D/eneros-data.raw"
run_window "$D" "$SEED_DL" "$D/eneros-data.raw"
wait_ready_and_checkpoints "$D/qemu-serial.log" 3 "$((SEED_DL * 2))" || {
  echo "[✗][seed] checkpoint 未达 3 次（盘上无持久状态）"; power_cut; exit 10; }
kill_fleet
S0="$D/qemu-serial.log"
SEED_PTS=$(grep -a "TSDB-DISK-OPEN" "$S0" | head -1)
echo "[seed] $SEED_PTS"

# ---- 段3 断电循环 ×2 --------------------------------------------------------
echo "=== 段3 断电循环（cycle1：checkpoint 完成后短切）==="
D1="$RUN/cut1"; mkdir -p "$D1"; cp -f "$D/eneros-data.raw" "$D1/eneros-data.raw" || { echo "seed 盘缺失"; exit 8; }
run_window "$D1" "$FINAL_DL" "$D1/eneros-data.raw"
# 等 PRODUCT-READY + 首次 checkpoint 完成，再短延时切电（落点≈checkpoint
# 间隙/下一次持久写中途——脏断电语义）。
if wait_ready_and_checkpoints "$D1/qemu-serial.log" 1 "$FINAL_DL"; then
  sleep "${CUT1_DELAY:-2}"
  power_cut
  echo "[cycle1] 已切电（checkpoint 后短延时）"
  grep -q "PRODUCT-READY: true" "$D1/qemu-serial.log" \
    && echo "[✓][cycle1] 切电前 PRODUCT-READY" \
    || { echo "[✗][cycle1] 切电前未 ready"; FAIL=1; }
else
  echo "[✗][cycle1] 窗内未达 ready+checkpoint（切电前状态不成立）"; power_cut; FAIL=1
fi

echo "=== 段3 断电循环（cycle2：同一盘直继，窗中段切）==="
D2="$RUN/cut2"; mkdir -p "$D2"
run_window "$D2" "$FINAL_DL" "$D1/eneros-data.raw"
if wait_ready_and_checkpoints "$D2/qemu-serial.log" 1 "$FINAL_DL"; then
  sleep "${CUT2_DELAY:-25}"
  power_cut
  echo "[cycle2] 已切电（窗中段）"
  grep -q "PRODUCT-READY: true" "$D2/qemu-serial.log" \
    && echo "[✓][cycle2] 切电前 PRODUCT-READY" \
    || { echo "[✗][cycle2] 切电前未 ready"; FAIL=1; }
else
  echo "[✗][cycle2] 窗内未达 ready+checkpoint"; power_cut; FAIL=1
fi

# ---- 段4 终窗恢复判据 --------------------------------------------------------
echo "=== 段4 终窗（同一盘直继，${FINAL_DL}s 恢复判据）==="
D3="$RUN/final"; mkdir -p "$D3"
run_window "$D3" "$FINAL_DL" "$D1/eneros-data.raw"
wait_ready_and_checkpoints "$D3/qemu-serial.log" 1 "$FINAL_DL" || {
  echo "[✗][R5] 脏断电×2 后 PRODUCT-READY+checkpoint 未达"; kill_fleet; FAIL=1; }
sleep "$FINAL_DL"
kill_fleet
S3="$D3/qemu-serial.log"
[ -f "$S3" ] || { echo "final serial missing"; exit 7; }

# R1 复用盘保数据：OPEN 走 mount 且旧点 >0。
OPEN_LINE=$(grep -a "TSDB-DISK-OPEN" "$S3" | head -1)
echo "[R1] final OPEN：$OPEN_LINE"
grep -aq "TSDB-DISK-OPEN" "$S3" || { echo "[✗][R1] 未走 TSDB-DISK-OPEN"; FAIL=1; }
POINTS=$(echo "$OPEN_LINE" | grep -o "points=[0-9]*" | grep -o "[0-9]*$")
[ -n "$POINTS" ] && [ "$POINTS" -gt 0 ] || { echo "[✗][R1] 旧点数=$POINTS（数据丢失）"; FAIL=1; }

# R2 全窗零 PANIC / 零分配失败（seed 外三段全文件，无豁免）。
PANICS=$(cat "$D1/qemu-serial.log" "$D2/qemu-serial.log" "$S3" \
  | grep -aci "PANIC\|memory allocation of" || true)
[ "$PANICS" -eq 0 ] && echo "[✓][R2] 脏断电各窗零 PANIC" \
  || { echo "[✗][R2] PANIC=$PANICS"; FAIL=1; }

# R3 控制链恢复进展（rounds 增长 + 遥测受理恢复）。
ROUNDS=$(grep -ac "CONTROL-CHAIN: round=" "$S3" || true)
[ "$ROUNDS" -ge "${MIN_ROUNDS:-60}" ] && echo "[✓][R3] 控制链恢复（rounds=$ROUNDS）" \
  || { echo "[✗][R3] rounds=$ROUNDS 不足"; FAIL=1; }
grep -aq "ingest rejected\|ingest transport" "$S3" \
  && echo "[i][R3] ingest 传输间歇存在（如实记录，判据不豁免）" || true

# R4 checkpoint 恢复（脏断电后持久化管线重启）。
CK=$(grep -ac "TSDB-CHECKPOINT-DUR" "$S3" || true)
[ "$CK" -ge 1 ] && echo "[✓][R4] checkpoint 恢复（×$CK）" \
  || { echo "[✗][R4] checkpoint 零恢复"; FAIL=1; }

# R5 PRODUCT-READY（终窗）。
grep -aq "PRODUCT-READY: true" "$S3" && echo "[✓][R5] 终窗 PRODUCT-READY" \
  || { echo "[✗][R5] 终窗未 ready"; FAIL=1; }

# R6 决策链身份零缺失。
grep -aq "CHAIN-KEY-MISSING" "$S3" && { echo "[✗][R6] CHAIN-KEY-MISSING 出现"; FAIL=1; } \
  || echo "[✓][R6] 零 CHAIN-KEY-MISSING"

# 证据归档：三段串口全文件 + 断电盘。
cp -f "$D1/qemu-serial.log" "$EVROOT/cut1.qemu-serial.log"
cp -f "$D2/qemu-serial.log" "$EVROOT/cut2.qemu-serial.log"
cp -f "$S3" "$EVROOT/final.qemu-serial.log"
cp -f "$D1/eneros-data.raw" "$EVROOT/powercut.eneros-data.raw" 2>/dev/null || true

if [ "$FAIL" -eq 0 ]; then
  echo "VOL70.2-R03-POWER-CUT GATE: PASS"
else
  echo "VOL70.2-R03-POWER-CUT GATE: FAIL"
fi
exit $FAIL
