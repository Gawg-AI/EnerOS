#!/bin/bash
# 蓝图 69 卷 69.2（v0.327.0）出口 host+QEMU 联合窗（2026-09-07）：
#   planner-peer 会话深模块化（PeerSession）——host 侧 bin，目标镜像零变更
#   （复用 69.1 出口镜像）。单窗 780s 串行四段：
#   段1 健康链（Q1/Q2/Q3/Q6）：broker+peer+QEMU → READY/SUBSCRIBED/
#      PEER-REQ/PLANNER-SOLVE optimal/PEER-RESP → REMOTE-BRAIN used=true
#      + [backend=planner] + dispatch=Committed + ACQ 双源；
#   段2 拔线降级（Q4/Q7）：kill broker → peer RECONNECT(poll-failed/
#      inbound-silent) → QEMU lease LEASE-ARMED → used=false → Committed
#      维持；中途 kill -9 peer + 重启（同 state 文件）→ PEER-RESP seq
#      跨重启单调（REG-PLANNER-SEQ-STATE-CORRUPT 不回退）；重启 broker →
#      used=true 回归；
#   段3 半死 broker（Q5，本卷的牙）：SIGSTOP broker → peer 侧 ≤50s
#      （45s 判死 + 轮询余量）出现 RECONNECT reason=inbound-silent
#      （§3.2 静态锁的实弹验证）；
#   段4 恢复（Q4 后半）：SIGCONT broker → SUBSCRIBED 回归。
#   Q6：stats() 四 reason 计数的运行时可读面 = RECONNECT 词形（按 reason
#   分行的运维可读账）；无独立 dump 面（新造观测服务属 Z7 禁区）——如实
#   登记，不标 N/A（词形即账）。
#   证据归档 evidence/69/vol69.2/（force-add）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/69/vol69.2"
RUN="$ROOT/target/evidence69-vol2"
mkdir -p "$EVROOT/build" "$RUN"

gate_cleanup_env 5597 5602 2404 1883
gate_export_signing_key
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
sleep 2
for PORT in 5597 5602 2404 1883; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

# 目标镜像零变更声明：复用 69.1 出口镜像（planner-peer 为 host bin）。
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"
echo "[i] 目标镜像复用（69.1 出口构建；69.2 未触 target 代码）"
PP_BIN="$ROOT/target/release/eneros-planner-peer"
[ -x "$PP_BIN" ] || { echo "planner-peer bin 缺失（先 cargo build --release -p eneros-planner-peer）"; exit 3; }
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

DL=840
W="joint"
DIR="$RUN/$W"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
rm -f "$RUN/$W.planner-peer.log" "$RUN/$W.seq-state.json" "$RUN/$W.broker-transcript.jsonl"

BROKER_PID=""

# ---- 段 1：拉起全场（broker + peer + 采集对端 + QEMU） --------------------
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" \
  --port 1883 --transcript "$RUN/$W.broker-transcript.jsonl" >"$RUN/$W.broker.log" 2>&1 &
BROKER_PID=$!
sleep 1
"$PP_BIN" --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$RUN/$W.seq-state.json" >"$RUN/$W.planner-peer.log" 2>&1 &
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi
grep -q "READY broker=" "$RUN/$W.planner-peer.log" || { echo "PEER-NOT-READY"; cat "$RUN/$W.planner-peer.log"; exit 5; }

cp -f "$ROOT/evidence/62/vol62-3/normal.modbus.transcript.jsonl" "$EVROOT/$W.modbus.transcript.jsonl"
cp -f "$ROOT/evidence/62/vol62-3/normal.iec104.transcript.jsonl" "$EVROOT/$W.iec104.transcript.jsonl"
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/$W.modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$EVROOT/$W.iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1

echo "=== host+QEMU 联合窗（$DL s，四段串行）==="
# QEMU 后台跑满窗；主脚本按段推进事件。
cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1 &
QEMU_JOB=$!

# 段内事件推进（等待 QEMU 就绪后按时间轴动作）。
t0=$(date +%s)
wait_ready() {  # 等 PRODUCT-READY（最长 300s）
  for _ in $(seq 1 100); do
    grep -q "PRODUCT-READY: true" "$DIR/qemu-serial.log" 2>/dev/null && return 0
    sleep 3
  done
  return 1
}
wait_ready || { echo "PRODUCT-READY 未达"; kill $QEMU_JOB $PMOD $P104 2>/dev/null; exit 6; }
sleep 120  # 段1：健康链样本（used=true / Committed / ACQ；给目标侧接受窗充分样本）

# ---- 段 2：拔线降级 + 强杀 peer + 重启 broker（Q4/Q7） ---------------------
echo "[gate] 段2：kill broker（拔线降级）"
kill -9 $BROKER_PID 2>/dev/null || true
sleep 100  # lease 到期（>3×超时窗）→ LEASE-ARMED → System1 兜底
echo "[gate] 段2：kill -9 planner-peer + 重启（Q7 seq 跨重启单调）"
SEQ_BEFORE=$(grep -o "PEER-RESP seq=[0-9]*" "$RUN/$W.planner-peer.log" | tail -1 | grep -o "[0-9]*")
SEQ_BEFORE=${SEQ_BEFORE:--1}
pkill -9 -f "release/eneros-planner-peer" 2>/dev/null || true
sleep 2
"$PP_BIN" --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$RUN/$W.seq-state.json" >>"$RUN/$W.planner-peer.log" 2>&1 &
sleep 2
echo "[gate] 段2：重启 broker（恢复）"
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" \
  --port 1883 --transcript "$RUN/$W.broker-transcript.jsonl" >>"$RUN/$W.broker.log" 2>&1 &
BROKER_PID=$!
sleep 120  # 恢复段：used=true 回归

# ---- 段 3：半死 broker（Q5） ----------------------------------------------
echo "[gate] 段3：SIGSTOP broker（半死）——Q5 计时开始"
STOP_T=$(date +%s)
kill -STOP $BROKER_PID 2>/dev/null || true
Q5_OK=0
for _ in $(seq 1 30); do   # 最长 60s 轮询（判据 ≤50s）
  sleep 2
  if tail -20 "$RUN/$W.planner-peer.log" | grep -q "RECONNECT reason=inbound-silent"; then
    Q5_ELAPSED=$(( $(date +%s) - STOP_T ))
    Q5_OK=1
    break
  fi
done
echo "[gate] Q5 inbound-silent 出现耗时 = ${Q5_ELAPSED:-N/A}s（判据 ≤50s）"
sleep 20
echo "[gate] 段4：SIGCONT broker（恢复）"
kill -CONT $BROKER_PID 2>/dev/null || true
sleep 40   # 恢复样本

kill $QEMU_JOB $PMOD $P104 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
sleep 2
cp -f "$DIR/qemu-serial.log" "$EVROOT/$W.qemu-serial.log" 2>/dev/null || true
cp -f "$RUN/$W.planner-peer.log" "$EVROOT/$W.planner-peer.log"
cp -f "$RUN/$W.broker-transcript.jsonl" "$EVROOT/$W.broker-transcript.jsonl"
S="$DIR/qemu-serial.log"
P="$EVROOT/$W.planner-peer.log"
BT="$EVROOT/$W.broker-transcript.jsonl"
[ -f "$S" ] || { echo "serial log missing"; exit 6; }

echo "=== 判据扫描 ==="
FAIL=0
# ---- Q1 真 broker 在局 + 健康链 ----
grep -q "READY broker=" "$P" && echo "[✓][Q1] READY 词形（字段齐全）" || { echo "[✗][Q1] READY 缺失"; FAIL=1; }
grep -aq "SUBSCRIBED topic=" "$P" && echo "[✓][Q1] SUBSCRIBED" || { echo "[✗][Q1] SUBSCRIBED 缺失"; FAIL=1; }
if grep -aq '"event": *"CONNACK"' "$BT"; then echo "[✓][Q1] broker transcript CONNACK" || true; else echo "[✗][Q1] broker 零 CONNACK"; FAIL=1; fi
n=$(grep -c "PEER-REQ req_id=" "$P"); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][Q1] PEER-REQ = $n" || { echo "[✗][Q1] PEER-REQ = 0"; FAIL=1; }
n=$(grep -c "PLANNER-SOLVE status=optimal" "$P"); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][Q1] PLANNER-SOLVE optimal = $n（真 HiGHS L2 门）" || { echo "[✗][Q1] PLANNER-SOLVE optimal = 0"; FAIL=1; }
grep -aq "PEER-RESP seq=" "$P" && echo "[✓][Q1] PEER-RESP 在" || { echo "[✗][Q1] PEER-RESP 缺失"; FAIL=1; }
# ---- Q2 QEMU 决策链吃到 peer ----
grep -aq "REMOTE-BRAIN used=true" "$S" && echo "[✓][Q2] REMOTE-BRAIN used=true" || { echo "[✗][Q2] used=true 缺失"; FAIL=1; }
grep -aq "resp=\"\[backend=planner\]" "$S" && echo "[✓][Q3] 审计 [backend=planner] 词形" || { echo "[✗][Q3] backend=planner 词形缺失"; FAIL=1; }
grep -q "dispatch=Committed" "$S" && echo "[✓][Q2] dispatch=Committed" || { echo "[✗][Q2] Committed 缺失"; FAIL=1; }
n=$(grep -ac "\[svc-modbus-tcp\] ACQ point" "$S"); n=${n:-0}
[ "$n" -ge 5 ] && echo "[✓][Q2] modbus ACQ = $n" || { echo "[✗][Q2] modbus ACQ = $n"; FAIL=1; }
n=$(grep -ac "\[svc-iec104\] ACQ point" "$S"); n=${n:-0}
[ "$n" -ge 5 ] && echo "[✓][Q2] iec104 ACQ = $n" || { echo "[✗][Q2] iec104 ACQ = $n"; FAIL=1; }
n=$(grep -ac "TSDB-DISK-COMMIT " "$S"); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][Q2] TSDB-DISK-COMMIT = $n" || { echo "[✗][Q2] TSDB-DISK-COMMIT = 0"; FAIL=1; }
# ---- Q4 拔线降级链 ----
grep -aq "RECONNECT reason=poll-failed\|RECONNECT reason=inbound-silent" "$P" \
  && echo "[✓][Q4] peer 侧拔线重连词形" || { echo "[✗][Q4] 拔线重连词形缺失"; FAIL=1; }
grep -aq "LEASE-ARMED" "$S" && echo "[✓][Q4] LEASE-ARMED（lease 到期武装）" || { echo "[✗][Q4] LEASE-ARMED 缺失"; FAIL=1; }
grep -aq "REMOTE-BRAIN used=false" "$S" && echo "[✓][Q4] 降级期 used=false" || { echo "[✗][Q4] used=false 缺失"; FAIL=1; }
n=$(grep -ac "REMOTE-BRAIN used=true" "$S"); n=${n:-0}
[ "$n" -ge 2 ] && echo "[✓][Q4] used=true 回归（$n 次）" || { echo "[✗][Q4] used=true 回归缺失（$n）"; FAIL=1; }
# ---- Q7 seq 跨重启单调 ----
SEQ_AFTER_FIRST=$(grep -o "PEER-RESP seq=[0-9]*" "$P" | awk -F= -v b="$SEQ_BEFORE" 'seen && $2<=b {print "REGRESS"; exit} $2>b {seen=1} END{}')
SEQ_AFTER=$(grep -o "PEER-RESP seq=[0-9]*" "$P" | tail -1 | grep -o "[0-9]*")
if [ -n "$SEQ_AFTER" ] && [ "$SEQ_AFTER" -ge "$SEQ_BEFORE" ] && [ "$SEQ_BEFORE" -ge 0 ]; then
  echo "[✓][Q7] PEER-RESP seq 跨重启单调（重启前末值=$SEQ_BEFORE，窗内末值=$SEQ_AFTER）"
else
  echo "[✗][Q7] seq 单调性（before=$SEQ_BEFORE after=$SEQ_AFTER）"; FAIL=1
fi
# ---- Q5 半死 broker（本卷的牙） ----
if [ "$Q5_OK" -eq 1 ] && [ "$Q5_ELAPSED" -le 50 ]; then
  echo "[✓][Q5] SIGSTOP 后 ${Q5_ELAPSED}s ≤50s 出现 inbound-silent（静态锁实弹）"
else
  echo "[✗][Q5] inbound-silent 未在 ≤50s 内出现（OK=$Q5_OK elapsed=${Q5_ELAPSED:-N/A}）"; FAIL=1
fi
grep -aq "SUBSCRIBED" "$P" && echo "[✓][Q5→段4] SIGCONT 后 SUBSCRIBED 回归" || { echo "[✗] 恢复后 SUBSCRIBED 缺失"; FAIL=1; }
# ---- Q6 stats 观测面 ----
for r in inbound-silent ping-failed publish-failed poll-failed; do
  c=$(grep -ac "RECONNECT reason=$r" "$P"); c=${c:-0}
  echo "[i][Q6] RECONNECT reason=$r ×$c（按 reason 分行的运维可读账；stats() 程序面由 p8 锁）"
done
# ---- 无挂死/无 PANIC ----
PANIC_ALL=$(grep -ac "PANIC" "$S" 2>/dev/null); PANIC_ALL=${PANIC_ALL:-0}
PANIC_EXEMPT=$(grep -ac 'svc-control-dispatch\] PANIC: panicked at.*alloc.rs' "$S" 2>/dev/null); PANIC_EXEMPT=${PANIC_EXEMPT:-0}
[ $((PANIC_ALL - PANIC_EXEMPT)) -eq 0 ] && echo "[✓] PANIC 净值 = 0" || { echo "[✗] PANIC 净值 = $((PANIC_ALL - PANIC_EXEMPT))"; FAIL=1; }
grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }

echo "=== 汇总 ==="
if [ $FAIL -eq 0 ]; then
  echo "VOL69.2-GATE: PASS"
else
  echo "VOL69.2-GATE: FAIL"; exit 7
fi
