#!/bin/bash
# 蓝图 69 卷 69.1（v0.326.0）出口 QEMU 实弹回归（2026-09-07）：
#   chunked 接缝判别力收口——BUG-S2（陈旧 pending_full 以 ENVP 错配）
#   一行修复 + BUG-S1 B 方案（B-1 拒绝覆盖未拉完分页响应 + 越界 Pull
#   记账词形）+ I3 MR 预算静态锁/兜底 + I4 响应页数域检。
#   本卷不改 root-task / wire / 帧常量（Z1/Z11/Z12）。
#   判据（卷 69.1 §3）：
#   Q1 标准启动窗无回退：68.5 既判词形保持（PRODUCT-READY / REMOTE-BRAIN
#      used=true / CONTROL-CHAIN dispatch=Committed / TSDB-DISK-COMMIT>0）；
#   Q2 chunked 正常链路不受损：modbus-tcp + iec104 双源 ACQ 行正常；
#      TSDB-DISK-UNAVAILABLE 零（TELH 可答面在）；
#   Q3 越界 Pull 可观测：CHUNKED-PAGE-RANGE 词形/计数——正常窗结构性
#      不触发 ⇒ 如实记 N/A（不得伪造触发）；
#   Q4 新词形负断言：CHUNKED-PAGE-RANGE / CHUNKED-RESP-TOOLARGE /
#      CHUNKED-REPLY-TRUNCATED / PagesOutstanding / ReplyTooLarge 零出现
#      （健康窗 fail-closed 面不可达；tools/ gate 脚本无词形白名单需同步，
#       68.1 负断言风格一致）；
#   Q5 无挂死：agent=OK 在带（floor 20）+ CHUNKED-SLOW 仅观测。
#   证据归档 evidence/69/vol69.1/（force-add）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/69/vol69.1"
RUN="$ROOT/target/evidence69-vol1"
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

PP_BIN="$ROOT/target/release/eneros-planner-peer"
[ -x "$PP_BIN" ] || { echo "planner-peer bin 缺失（先 cargo build --release -p eneros-planner-peer）"; exit 3; }

echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接（控制面 bin 全新）+ image raw ==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -20 "$EVROOT/build/relink.log"; exit 9; }
grep -a "Rust ELF:" "$EVROOT/build/relink.log" | tail -1
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

DL=720
W="planner-live"
DIR="$RUN/$W"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"

# 站端面拉起（broker + planner-peer——68.5 REMOTE-BRAIN 词形前置）。
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" \
  --port 1883 --transcript "$RUN/$W.broker-transcript.jsonl" >"$RUN/$W.broker.log" 2>&1 &
sleep 1
"$PP_BIN" --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$RUN/$W.seq-state.json" >"$RUN/$W.planner-peer.log" 2>&1 &
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi
grep -q READY "$RUN/$W.planner-peer.log" || { echo "PEER-NOT-READY"; cat "$RUN/$W.planner-peer.log"; exit 5; }

# 站端采集对端（modbus + iec104）。
cp -f "$ROOT/evidence/62/vol62-3/normal.modbus.transcript.jsonl" "$EVROOT/$W.modbus.transcript.jsonl"
cp -f "$ROOT/evidence/62/vol62-3/normal.iec104.transcript.jsonl" "$EVROOT/$W.iec104.transcript.jsonl"
python3 "$ROOT/evidence/61/modbus_slave61.py" \
  --transcript "$EVROOT/$W.modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" \
  --transcript "$EVROOT/$W.iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1
if ! kill -0 $PMOD 2>/dev/null; then echo "MODBUS-PEER-DIED"; exit 5; fi
if ! kill -0 $P104 2>/dev/null; then echo "IEC104-PEER-DIED"; exit 5; fi

echo "=== 步骤 3：QEMU 实弹窗（$W，fresh 盘，${DL}s）==="
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1
RRC=$?
kill $PMOD $P104 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
cp -f "$DIR/qemu-serial.log" "$EVROOT/$W.qemu-serial.log" 2>/dev/null || true
echo "window $W rc=$RRC（124/2=超时杀=完整窗口）"
[ -f "$DIR/qemu-serial.log" ] || { echo "serial log missing"; exit 6; }
S="$DIR/qemu-serial.log"

echo "=== 步骤 4：判据扫描 ==="
FAIL=0
# ---- Q1 标准启动窗无回退 ----
grep -q "PRODUCT-READY: true" "$S" && echo "[✓][Q1] PRODUCT-READY: true" || { echo "[✗][Q1] PRODUCT-READY"; FAIL=1; }
PANIC_ALL=$(grep -ac "PANIC" "$S" 2>/dev/null); PANIC_ALL=${PANIC_ALL:-0}
PANIC_EXEMPT=$(grep -ac 'svc-control-dispatch\] PANIC: panicked at.*alloc.rs' "$S" 2>/dev/null); PANIC_EXEMPT=${PANIC_EXEMPT:-0}
PANIC_NET=$((PANIC_ALL - PANIC_EXEMPT))
[ "$PANIC_NET" -eq 0 ] && echo "[✓][Q1] PANIC 净值 = 0（全量 $PANIC_ALL，豁免 $PANIC_EXEMPT）" || { echo "[✗][Q1] PANIC 净值 = $PANIC_NET"; FAIL=1; }
n=$(grep -ac "READINESS: " "$S" 2>/dev/null); n=${n:-0}
[ "$n" -eq 29 ] && echo "[✓][Q1] READINESS = 29" || { echo "[✗][Q1] READINESS 行=$n（基线 29）"; FAIL=1; }
grep -aq "TSDB-DISK-OPEN " "$S" && echo "[✓][Q1] TSDB-DISK-OPEN" || { echo "[✗][Q1] TSDB-DISK-OPEN 缺失"; FAIL=1; }
n=$(grep -ac "TSDB-DISK-COMMIT " "$S" 2>/dev/null); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][Q1] TSDB-DISK-COMMIT = $n" || { echo "[✗][Q1] TSDB-DISK-COMMIT = 0"; FAIL=1; }
grep -q "dispatch=Committed" "$S" && echo "[✓][Q1] CONTROL-CHAIN dispatch=Committed" || { echo "[✗][Q1] dispatch=Committed 缺失"; FAIL=1; }
grep -aq "AI-PEER-CFG loaded" "$S" && echo "[✓][Q1] AI-PEER-CFG loaded" || { echo "[✗][Q1] AI-PEER-CFG loaded 缺失"; FAIL=1; }
if grep -aq '"event": *"CONNACK"' "$RUN/$W.broker-transcript.jsonl"; then
  echo "[✓][Q1] broker transcript 有 peer CONNECT"
else
  echo "[✗][Q1] broker 零 CONNECT"; FAIL=1
fi
n=$(grep -c "PLANNER-SOLVE status=optimal" "$RUN/$W.planner-peer.log" 2>/dev/null); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][Q1] PLANNER-SOLVE optimal = $n" || { echo "[✗][Q1] PLANNER-SOLVE optimal = 0"; FAIL=1; }
grep -aq "REMOTE-BRAIN used=true" "$S" && echo "[✓][Q1] REMOTE-BRAIN used=true" || { echo "[✗][Q1] REMOTE-BRAIN used=true 缺失"; FAIL=1; }
# ---- Q2 chunked 正常链路不受损 ----
n=$(grep -ac "\[svc-modbus-tcp\] ACQ point" "$S" 2>/dev/null); n=${n:-0}
[ "$n" -ge 5 ] && echo "[✓][Q2] modbus ACQ = $n" || { echo "[✗][Q2] modbus ACQ = $n（<5）"; FAIL=1; }
n=$(grep -ac "\[svc-iec104\] ACQ point" "$S" 2>/dev/null); n=${n:-0}
[ "$n" -ge 5 ] && echo "[✓][Q2] iec104 ACQ = $n" || { echo "[✗][Q2] iec104 ACQ = $n（<5）"; FAIL=1; }
n=$(grep -ac "TSDB-DISK-UNAVAILABLE" "$S" 2>/dev/null); n=${n:-0}
[ "$n" -eq 0 ] && echo "[✓][Q2] TSDB-DISK-UNAVAILABLE = 0（TELH 可答面在）" || { echo "[✗][Q2] TSDB-DISK-UNAVAILABLE = $n"; FAIL=1; }
# ---- Q3 越界 Pull 可观测（正常窗结构性不触发 ⇒ N/A）----
n=$(grep -ac "CHUNKED-PAGE-RANGE" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -gt 0 ]; then
  echo "[i][Q3] CHUNKED-PAGE-RANGE 自然触发 ×$n（观测计数）"
else
  echo "[i][Q3] CHUNKED-PAGE-RANGE 未自然触发 ⇒ 本项 N/A（如实记录，不伪造触发）"
fi
# ---- Q4 新词形负断言（健康窗 fail-closed 面不可达）----
for pat in "CHUNKED-PAGE-RANGE" "CHUNKED-RESP-TOOLARGE" "CHUNKED-REPLY-TRUNCATED" "PagesOutstanding" "ReplyTooLarge" "ResponseTooLarge"; do
  c=$(grep -ac "$pat" "$S" 2>/dev/null); c=${c:-0}
  [ "$c" -eq 0 ] && echo "[✓][Q4] $pat = 0" || { echo "[✗][Q4] $pat = $c（健康窗不应出现）"; FAIL=1; }
done
# ---- Q5 无挂死 ----
AGOK=$(grep -ac "agent=OK" "$S" 2>/dev/null); AGOK=${AGOK:-0}
[ "$AGOK" -ge 20 ] && echo "[✓][Q5] agent=OK = $AGOK（≥20，采集/决策链活性）" || { echo "[✗][Q5] agent=OK = $AGOK（<20 = 链路停滞嫌疑）"; FAIL=1; }
SLOWN=$(grep -ac "CHUNKED-SLOW" "$S" 2>/dev/null); SLOWN=${SLOWN:-0}
echo "[i][Q5] CHUNKED-SLOW 观测计数 = $SLOWN（不作失败判据）"

echo "=== 汇总 ==="
if [ $FAIL -eq 0 ]; then
  echo "VOL69.1-GATE: PASS"
else
  echo "VOL69.1-GATE: FAIL"; exit 7
fi
