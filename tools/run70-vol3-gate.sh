#!/bin/bash
# 蓝图 70 卷 70.3（v0.333.0）出口门（WP-70.3.5）——IPC 有界组装、驱动
# 长度与响应判别的目标入口合同。
#
#   段0  判据器负例自检 + host 系统性输入矩阵：
#        - agent-sdk chunked（Header 分配前容量合同边界全扫 + v2 判别帧）；
#        - control-services service_slots（进程字节预算 / txn 事务归属 /
#          孤儿·空闲槽回收 / 既有 68.1 交错锁）；
#        - drv-svc runtime（PAYLOAD_CAP 声明域运行时拒绝 + 负例后队列可用）。
#        纪律：`-- --list` 先行确认过滤器非零（命中 0 = 验证失败），
#        再以同一过滤器实跑（方向 2 §6）。
#   段1  目标实弹窗（qemu-prod + modbus/iec104 从站 + MQTT broker 编队，
#        peer 离线 = 既有压力形）：v2 分页 wire 全量承载——每轮控制链
#        270B 分块请求 + ENVR 判别页拉取、svc-config 启动配置分发。
#        判据：
#        R1  judge G1–G7（零 PANIC/零分配失败无豁免、相位健康、轮数、
#            对端 transcript 活性）；
#        R2  协议配置分发面 ≥ 基线（PROTOCOL-CONFIG loaded ≥ 6——7099B
#            级分页响应在 v2 wire 下的目标实弹）；
#        R3  70.3 拒绝词形硬零：CHUNKED-BUDGET / CHUNKED-RESP-TOOLARGE /
#            CHUNKED-REPLY-TRUNCATED / CHUNKED-PULL-STALE / CHUNKED-SLOT-FULL /
#            ERR:BadTotalLen / ERR:BadChunkSize——健康窗合法流量永不触发；
#        R4  观测计数（不判负）：DUP-SESSION / PAGE-EVICTED / PAGE-RANGE /
#            SLOW——预存语义（客户端重试自愈、慢盘停顿），逐项如实记录。
#   用法：DL=720 MIN_ROUNDS=400 tools/run70-vol3-gate.sh
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/70/70.3"
RUN="$ROOT/target/evidence70-vol3"
DL="${DL:-720}"
MIN_ROUNDS="${MIN_ROUNDS:-400}"
PROTO_CFG_MIN="${PROTO_CFG_MIN:-6}"
mkdir -p "$EVROOT" "$RUN" "$EVROOT/host"

FAIL=0

# ---- 段0：判据器自检 + host 输入矩阵 ------------------------------------
python3 "$ROOT/tools/run70_gate_judge.py" --selftest || {
  echo "判据器自检 FAIL——门不可信"; exit 2; }

run_filter() {  # $1=crate $2=filter（支持含空格的路径过滤器）
  local CRATE="$1"; shift
  local FILTER="$*"
  local N
  N=$(cargo test --offline --locked -p "$CRATE" --lib $FILTER -- --list 2>/dev/null | grep -c ': test$')
  if [ "$N" -eq 0 ]; then
    echo "[✗] host $CRATE '$FILTER' 过滤器 0 项——验证失败"
    FAIL=1; return
  fi
  echo "[host] $CRATE '$FILTER'：$N 用例"
  if ! cargo test --offline --locked -p "$CRATE" --lib $FILTER >"$EVROOT/host/${CRATE}_$(echo $FILTER | tr ' :' '__').log" 2>&1; then
    echo "[✗] host $CRATE '$FILTER' 实跑失败"
    tail -5 "$EVROOT/host/${CRATE}_$(echo $FILTER | tr ' :' '__').log"
    FAIL=1
  fi
}
run_filter eneros-agent-sdk chunked
run_filter eneros-control-services service_slots
run_filter eneros-drv-svc runtime
[ "$FAIL" -eq 0 ] || { echo "=== 段0 host 输入矩阵 FAIL ==="; exit 9; }

# ---- 段1：镜像构建（target bin 携带 chunked/service_slots/drv-svc 变更）--
gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
gate_export_signing_key
gate_compile_config "$RUN/config-compile.log" qemu-prod
echo "=== sel4-build-rust 重链接（root/服务闭包变更）==="
gate_relink_cached "$RUN/sel4.log" || {
  echo "SEL4-RELINK FAIL"; tail -20 "$RUN/sel4.log"; exit 3; }
gate_image_raw_cached qemu-prod "$RUN/image.log"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

DIR="$RUN/win"; mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw" \
  "$DIR/modbus.transcript.jsonl" "$DIR/iec104.transcript.jsonl" "$DIR/broker-transcript.jsonl"

echo "=== 段1 目标实弹窗（$DL s；peer 离线压力形；门限 $MIN_ROUNDS 轮）==="
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$DIR/modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$DIR/iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$DIR/broker-transcript.jsonl" >"$DIR/broker.log" 2>&1 &
PBRK=$!
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi

cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1 &
QEMU_JOB=$!

wait_ready() {
  for _ in $(seq 1 100); do
    grep -q "PRODUCT-READY: true" "$DIR/qemu-serial.log" 2>/dev/null && return 0
    sleep 3
  done
  return 1
}
wait_ready || { echo "PRODUCT-READY 未达"; kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null || true; sleep 2; kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true; exit 6; }
sleep "$DL"
kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null || true
sleep 3
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave62") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true

S="$DIR/qemu-serial.log"
[ -f "$S" ] || { echo "serial log missing"; exit 7; }
grep -q "PRODUCT-READY: true" "$S" || { echo "[✗] 未 PRODUCT-READY"; FAIL=1; }
cp -f "$S" "$EVROOT/qemu-serial.log"
cp -f "$DIR/broker-transcript.jsonl" "$EVROOT/broker-transcript.jsonl" 2>/dev/null || true

# ---- R1：判据器 G1–G7 ----------------------------------------------------
python3 "$ROOT/tools/run70_gate_judge.py" "$S" "$MIN_ROUNDS" \
  --window-secs "$DL" \
  --transcripts "$DIR/modbus.transcript.jsonl" "$DIR/iec104.transcript.jsonl" \
  | tee "$EVROOT/judge.json" || FAIL=1

# ---- R2：协议配置分发面（v2 分页响应目标实弹）----------------------------
PCFG=$(grep -ac "PROTOCOL-CONFIG loaded" "$S" || true)
echo "[R2] PROTOCOL-CONFIG loaded 行=$PCFG（基线 ≥$PROTO_CFG_MIN）"
[ "$PCFG" -ge "$PROTO_CFG_MIN" ] || { echo "[✗][R2] 配置分发面不足（7099B 级分页响应面受损）"; FAIL=1; }

# ---- R3：70.3 拒绝词形硬零 ------------------------------------------------
for WORD in CHUNKED-BUDGET CHUNKED-RESP-TOOLARGE CHUNKED-REPLY-TRUNCATED \
            CHUNKED-PULL-STALE CHUNKED-SLOT-FULL "ERR:BadTotalLen" "ERR:BadChunkSize"; do
  N=$(grep -ac "$WORD" "$S" || true)
  if [ "$N" -gt 0 ]; then
    echo "[✗][R3] 健康窗出现拒绝词形 $WORD ×$N"; FAIL=1
  else
    echo "[R3] $WORD ×0"
  fi
done

# ---- R4：观测计数（如实记录，不判负）--------------------------------------
for WORD in CHUNKED-DUP-SESSION CHUNKED-PAGE-EVICTED CHUNKED-PAGE-RANGE CHUNKED-SLOW; do
  N=$(grep -ac "$WORD" "$S" || true)
  echo "[R4] 观测 $WORD ×$N"
done
ROUNDS=$(grep -ac "CONTROL-CHAIN: round=" "$S" || true)
echo "[R4] CONTROL-CHAIN rounds=$ROUNDS"

if [ "$FAIL" -eq 0 ]; then
  echo "=== VOL70.3 GATE: PASS ==="
else
  echo "=== VOL70.3 GATE: FAIL ==="
  exit 11
fi
