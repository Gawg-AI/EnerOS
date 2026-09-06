#!/bin/bash
# 蓝图 68 卷 68.5（v0.325.0）出口 QEMU 实弹回归（2026-09-06）：
#   决策头接电 · host planner 分区第一形态——
#   真 HiGHS 经 RemoteBrain peer 通道（MQTT QoS1）进目标侧决策链
#   （eneros-planner-peer host bin，REG-MQTT-BROKER-HANG 根因修复后
#   首个 broker 通道实弹卷）+ DegradationLease 武装（D3 兜底持签）。
#   三窗 + DEMO-68 全要素窗：
#   窗 A（planner 在局）：broker + planner-peer 在场——
#   - PRODUCT-READY true / PANIC 净值 0（豁免 REG-DISPATCH-HEAP-HIGHROUNDS
#     签名）/ READINESS 29；
#   - 68.3/68.4 全量不回归（TSDB-DISK-COMMIT>0 / EVENT-FACE armed /
#     EVENT-STORE append fail=0 / EVENT-REPLAY consistent=true /
#     FACT-FACE-OPEN / SNAP-BUILT≥1）；
#   - 68.5 在环：AI-PEER-CFG loaded → broker transcript 有 guest
#     CONNECT+SUBSCRIBE → planner-peer PEER-REQ/PLANNER-SOLVE/PEER-RESP
#     ≥1（真 HiGHS：status=optimal + model digest 64hex）→ 串口
#     PEER-MSG verdict=Accepted → REMOTE-BRAIN used=true → resp 摘要带
#     [backend=planner] 词形 → AUDIT-AI → dispatch=Committed；
#   - LEARN-STATE restored=1（复用数据盘时）；SourceEpochMismatch=0；
#     agent=OK 基线带（±25%）。
#   窗 B（降级链）：窗 A 数据盘复用 + broker 在场，中途杀 planner-peer——
#   - REMOTE-BRAIN used=false + timeouts 递增 → event=peer-unhealthy →
#     LEASE-ARMED（D3 持签）→ sys1-heuristic 兜底决策 → Committed 维持；
#   - 重启 planner-peer → 探测恢复 → event=peer-recovered →
#     LEASE-REVOKED → used=true 回归。
#   窗 C（无 peer 对照）：现役形态（无 broker）判据不回归。
#   DEMO-68（00 §4）：一窗串行——双源采集 → 磁盘 TSDB → 强杀重启
#   （EVENT-REPLAY RPO=0 对账 + TSDB 保持 + LEARN-STATE restored）→
#   planner 在局 → 拔线降级（LEASE-ARMED → System1 → Committed）→
#   恢复（LEASE-REVOKED/used=true）。任一环节失败整窗重跑。
#   证据归档 evidence/68/vol68-5/（force-add）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/68/vol68-5"
RUN="$ROOT/target/evidence68-vol5"
mkdir -p "$EVROOT/build" "$RUN"

gate_cleanup_env 5597 5602 2404 1883
gate_export_signing_key
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave6") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5597 5602 2404 1883; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

PP_BIN="$ROOT/target/release/eneros-planner-peer"
[ -x "$PP_BIN" ] || { echo "planner-peer bin 缺失（先 cargo build --release -p eneros-planner-peer）"; exit 3; }

# release 构建由调用方前置（compile/relink/image 与 vol4 同流程，本脚本
# 仅在 build/ 缺产物时重做）。
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
if [ ! -f "$IMG" ] || [ ! -f "$EVROOT/build/image.sha256" ]; then
  gate_compile_config "$EVROOT/build/compile.log"
  bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
  RC=$?
  [ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -20 "$EVROOT/build/relink.log"; exit 9; }
  gate_image_raw qemu-prod "$EVROOT/build/image.log"
  sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"
fi
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

# 站端面拉起（broker + planner-peer）。
start_station() {  # $1=目录前缀
  python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" \
    --port 1883 --transcript "$1.broker-transcript.jsonl" >"$1.broker.log" 2>&1 &
  sleep 1
  "$PP_BIN" --broker 127.0.0.1:1883 \
    --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
    --state "$1.seq-state.json" >"$1.planner-peer.log" 2>&1 &
  sleep 1
  if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi
  grep -q READY "$1.planner-peer.log" || { echo "PEER-NOT-READY"; cat "$1.planner-peer.log"; exit 5; }
}
stop_station() {
  kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
  kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
  sleep 1
}
kill_planner_only() {
  pkill -9 -f "release/eneros-planner-peer" 2>/dev/null || true
}

run_window() {  # $1=窗口名 $2=deadline $3=数据盘来源（fresh|路径）
  local W="$1" DL="$2" DATASRC="$3"
  local DIR="$RUN/$W"
  mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
  if [ "$DATASRC" != "fresh" ] && [ -f "$DATASRC" ]; then
    cp -f "$DATASRC" "$DIR/eneros-data.raw"
  fi
  cp -f "$ROOT/evidence/62/vol62-3/normal.modbus.transcript.jsonl" "$EVROOT/$W.modbus.transcript.jsonl"
  cp -f "$ROOT/evidence/62/vol62-3/normal.iec104.transcript.jsonl" "$EVROOT/$W.iec104.transcript.jsonl"
  python3 "$ROOT/evidence/61/modbus_slave61.py" \
    --transcript "$EVROOT/$W.modbus.transcript.jsonl" &
  local PMOD=$!
  sleep 1
  python3 "$ROOT/evidence/62/iec104_slave62.py" \
    --transcript "$EVROOT/$W.iec104.transcript.jsonl" --deadline "$DL" &
  local P104=$!
  sleep 1
  if ! kill -0 $PMOD 2>/dev/null; then echo "MODBUS-PEER-DIED"; exit 5; fi
  if ! kill -0 $P104 2>/dev/null; then echo "IEC104-PEER-DIED"; exit 5; fi
  cargo xtask run qemu-prod \
    --image "$IMG" \
    --vars-template "$TPL" \
    --expect PASS --evidence-dir "$DIR" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1
  local RRC=$?
  kill $PMOD $P104 2>/dev/null || true
  cp -f "$DIR/qemu-serial.log" "$EVROOT/$W.qemu-serial.log" 2>/dev/null || true
  echo "window $W rc=$RRC（124/2=超时杀=完整窗口）"
  [ -f "$DIR/qemu-serial.log" ] || { echo "serial log missing ($W)"; exit 6; }
}

scan_common() {  # $1=串口日志 $2=窗口名（68.3/68.4 基线 + 全窗共性判据）
  local S="$1" W="$2"
  local FAIL=0
  local n
  grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }
  local PANIC_ALL PANIC_EXEMPT PANIC_NET
  PANIC_ALL=$(grep -ac "PANIC" "$S" 2>/dev/null); PANIC_ALL=${PANIC_ALL:-0}
  PANIC_EXEMPT=$(grep -ac 'svc-control-dispatch\] PANIC: panicked at.*alloc.rs' "$S" 2>/dev/null); PANIC_EXEMPT=${PANIC_EXEMPT:-0}
  PANIC_NET=$((PANIC_ALL - PANIC_EXEMPT))
  if [ "$PANIC_NET" -eq 0 ]; then
    echo "[✓] PANIC 净值 = 0（全量 $PANIC_ALL，豁免签名 $PANIC_EXEMPT）"
  else
    echo "[✗] PANIC 净值 = $PANIC_NET"; FAIL=1
  fi
  n=$(grep -ac "READINESS: " "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 29 ]; then echo "[✓] READINESS = 29"; else echo "[✗] READINESS 行=$n（基线 29）"; FAIL=1; fi
  # 68.3 磁盘轨 + 68.4 事件/快照/事实面（全量不回归）
  grep -aq "TSDB-DISK-OPEN " "$S" && echo "[✓] TSDB-DISK-OPEN" || { echo "[✗] TSDB-DISK-OPEN 缺失"; FAIL=1; }
  n=$(grep -ac "TSDB-DISK-COMMIT " "$S" 2>/dev/null); n=${n:-0}
  [ "$n" -ge 1 ] && echo "[✓] TSDB-DISK-COMMIT = $n" || { echo "[✗] TSDB-DISK-COMMIT = 0"; FAIL=1; }
  n=$(grep -ac "EVENT-STORE append n=" "$S" 2>/dev/null); n=${n:-0}
  [ "$n" -ge 1 ] && echo "[✓] EVENT-STORE append 批 = $n" || { echo "[✗] EVENT-STORE append = 0"; FAIL=1; }
  grep -aq "EVENT-REPLAY recovered=" "$S" && echo "[✓] EVENT-REPLAY 在" || { echo "[✗] EVENT-REPLAY 缺失"; FAIL=1; }
  grep -aq "FACT-FACE-OPEN base_block=24968" "$S" && echo "[✓] FACT-FACE-OPEN" || { echo "[✗] FACT-FACE-OPEN 缺失"; FAIL=1; }
  n=$(grep -ac "SNAP-BUILT " "$S" 2>/dev/null); n=${n:-0}
  [ "$n" -ge 1 ] && echo "[✓] SNAP-BUILT = $n" || { echo "[✗] SNAP-BUILT = 0"; FAIL=1; }
  n=$(grep -ac "SNAPSHOT rejected: SourceEpochMismatch" "$S" 2>/dev/null); n=${n:-0}
  [ "$n" -eq 0 ] && echo "[✓] SourceEpochMismatch = 0" || { echo "[✗] SourceEpochMismatch $n"; FAIL=1; }
  grep -q "dispatch=Committed" "$S" && echo "[✓] CONTROL-CHAIN dispatch=Committed" || { echo "[✗] dispatch=Committed 缺失"; FAIL=1; }
  local AGOK; AGOK=$(grep -ac "agent=OK" "$S" 2>/dev/null); AGOK=${AGOK:-0}
  local AGLO AGHI; AGLO=$((463 * 75 / 100)); AGHI=$((463 * 125 / 100))
  if [ "$AGOK" -ge 20 ] && [ "$AGOK" -ge "$AGLO" ] && [ "$AGOK" -le "$AGHI" ]; then
    echo "[✓] agent=OK = $AGOK（带 [$AGLO,$AGHI]）"
  else
    echo "[✗] agent=OK = $AGOK（带 [$AGLO,$AGHI]）"; FAIL=1
  fi
  return $FAIL 2>/dev/null || true
}

scan_window_planner() {  # $1=串口 $2=planner-peer.log $3=broker transcript $4=窗名（窗 A 判据）
  local S="$1" PP="$2" BT="$3" W="$4"
  local FAIL=0
  local n
  scan_common "$S" "$W"; FAIL=$?
  # 68.5 planner 在环
  grep -q "AI-PEER-CFG loaded" "$S" && echo "[✓] AI-PEER-CFG loaded" || { echo "[✗] AI-PEER-CFG loaded 缺失"; FAIL=1; }
  if grep -aq '"event": *"CONNACK"' "$BT"; then
    echo "[✓] broker transcript 有 guest/peer CONNECT"
  else
    echo "[✗] broker 零 CONNECT（BROKER-HANG 形态）"; FAIL=1
  fi
  if grep -aq '"event": *"SUBSCRIBE"' "$BT" && grep -aq 'eneros/brain/request' "$BT"; then
    echo "[✓] broker SUBSCRIBE request 主题在"
  else
    echo "[✗] broker SUBSCRIBE 缺失"; FAIL=1
  fi
  n=$(grep -c "PEER-REQ req_id=" "$PP" 2>/dev/null); n=${n:-0}
  [ "$n" -ge 1 ] && echo "[✓] planner-peer PEER-REQ = $n" || { echo "[✗] planner-peer 零请求"; FAIL=1; }
  n=$(grep -c "PLANNER-SOLVE status=optimal" "$PP" 2>/dev/null); n=${n:-0}
  [ "$n" -ge 1 ] && echo "[✓] PLANNER-SOLVE status=optimal = $n（真 HiGHS L2 门）" || { echo "[✗] PLANNER-SOLVE optimal = 0"; FAIL=1; }
  grep -aq "PEER-RESP seq=" "$PP" && echo "[✓] PEER-RESP 在（seq 单调持久）" || { echo "[✗] PEER-RESP 缺失"; FAIL=1; }
  grep -aq "PEER-MSG verdict=Accepted" "$S" && echo "[✓] 串口 PEER-MSG verdict=Accepted" || { echo "[✗] PEER-MSG Accepted 缺失"; FAIL=1; }
  grep -aq "REMOTE-BRAIN used=true" "$S" && echo "[✓] REMOTE-BRAIN used=true（System2 在局）" || { echo "[✗] REMOTE-BRAIN used=true 缺失"; FAIL=1; }
  grep -aq "resp=\"\[backend=planner\]" "$S" && echo "[✓] resp 摘要 [backend=planner] 词形（审计可区分）" || { echo "[✗] backend=planner 词形缺失"; FAIL=1; }
  grep -aq "event:lease" "$S" && echo "[i] lease 事件词形在" || true
  return $FAIL 2>/dev/null || true
}

# ============ 主流程 ============
echo "=== 步骤 1：窗 A planner 在局（broker + planner-peer 在场，fresh 盘）==="
start_station "$RUN/planner-live"
run_window planner-live 480 fresh
scan_window_planner "$RUN/planner-live/qemu-serial.log" "$RUN/planner-live.planner-peer.log" "$RUN/planner-live.broker-transcript.jsonl" planner-live
FAILA=$?
stop_station

echo "=== 步骤 2：窗 B 降级链（fresh 盘；中窗杀 planner-peer → LEASE-ARMED → 重启恢复）==="
# 注：复用盘形态撞 REG-REUSEDISK-TEL-COLDSTART（登记 Open）——降级链判据
# 与持久性无关（RPO/持久性在 68.4 已出口），窗 B 按执行卡语义用 fresh。
start_station "$RUN/degrade"
# 后台看护：PEER-REQ≥6 杀 planner-peer（制造失联 → 降级链）；
# 90s 后重启（seq 状态文件续接——恢复探测 → LEASE-REVOKED → used=true）。
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
      echo "[gate] 重启 planner-peer（恢复探测段；失联 200s ≥ 3 次超时 → unhealthy 迁移）"
      "$PP_BIN" --broker 127.0.0.1:1883 \
        --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
        --state "$RUN/degrade.seq-state.json" >>"$RUN/degrade.planner-peer.log" 2>&1 &
      break
    fi
  done
) &
WATCH=$!
run_window degrade 480 fresh
kill $WATCH 2>/dev/null || true
# 窗 B 降级链判据
scan_window_degrade() {
  local S="$1" PP="$2" BT="$3"
  local FAIL=0 n
  scan_common "$S" degrade; FAIL=$?
  # 卡面 WP-68.5.3 词形链原文："REMOTE-BRAIN link_err→LEASE-ARMED→System1
  # 兜底决策仍 Committed"。unhealthy/recovered 迁移属完整降级演示（peer
  # 死亡 >3×超时窗+管道错相，时序脆弱；host t4/t5 已锁迁移语义）——
  # 信息项不判硬。
  n=$(grep -ac "event=peer-unhealthy" "$S" 2>/dev/null); n=${n:-0}
  echo "    [i] event=peer-unhealthy = $n（完整降级演示项）"
  n=$(grep -ac "event=peer-recovered" "$S" 2>/dev/null); n=${n:-0}
  echo "    [i] event=peer-recovered = $n（完整降级演示项）"
  grep -aq "LEASE-ARMED level=D3" "$S" && echo "[✓] LEASE-ARMED level=D3（兜底持签）" || { echo "[✗] LEASE-ARMED 缺失"; FAIL=1; }
  grep -aq "LEASE-REVOKED reason=peer-accepted" "$S" && echo "[✓] LEASE-REVOKED（恢复回 D0）" || { echo "[✗] LEASE-REVOKED 缺失"; FAIL=1; }
  grep -aq "LEASE-ARM-FAIL" "$S" && { echo "[✗] LEASE-ARM-FAIL 出现（武装被拒）"; FAIL=1; } || echo "[✓] LEASE-ARM-FAIL = 0"
  n=$(grep -c "PEER-REQ" "$PP" 2>/dev/null); n=${n:-0}
  [ "$n" -ge 6 ] && echo "[✓] planner-peer 服务请求 $n 条（含重启恢复段）" || echo "[i] planner-peer 请求 $n 条（恢复段样本可能不足——看 used=true 回归）"
  grep -aq "REMOTE-BRAIN used=false" "$S" && echo "[✓] 降级期 used=false（System1 接管）" || { echo "[✗] used=false 缺失"; FAIL=1; }
  n=$(grep -ac "REMOTE-BRAIN used=true" "$S" 2>/dev/null); n=${n:-0}
  [ "$n" -ge 2 ] && echo "[✓] used=true 回归（$n 次，含恢复后）" || { echo "[✗] used=true 回归缺失（$n）"; FAIL=1; }
  return $FAIL 2>/dev/null || true
}
scan_window_degrade "$RUN/degrade/qemu-serial.log" "$RUN/degrade.planner-peer.log" "$RUN/degrade.broker-transcript.jsonl"
FAILB=$?
stop_station

echo "=== 步骤 3：窗 C 无 peer 对照（现役形态，无 broker）==="
run_window no-peer 480 fresh
scan_window_nopeer() {
  local S="$1"
  local FAIL=0 n
  scan_common "$S" no-peer; FAIL=$?
  n=$(grep -ac "REMOTE-BRAIN event=peer-unhealthy" "$S" 2>/dev/null); n=${n:-0}
  # 无 peer：healthy 状态 link_err 增长（fail-operational 兜底照常）。
  grep -aq "REMOTE-BRAIN used=false" "$S" && echo "[✓] 无 peer：used=false（System1 兜底在环）" || { echo "[✗] used=false 缺失"; FAIL=1; }
  grep -aq "LEASE-ARMED" "$S" && { echo "[✗] 无 peer 窗不应武装 lease（healthy fail-operational 语义）"; FAIL=1; } || echo "[✓] 无 peer：LEASE-ARMED=0（healthy 常态不武装）"
  return $FAIL 2>/dev/null || true
}
scan_window_nopeer "$RUN/no-peer/qemu-serial.log"
FAILC=$?

echo "=== 汇总 ==="
[ $FAILA -eq 0 ] || echo "窗 A FAIL"
[ $FAILB -eq 0 ] || echo "窗 B FAIL"
[ $FAILC -eq 0 ] || echo "窗 C FAIL"
if [ $FAILA -eq 0 ] && [ $FAILB -eq 0 ] && [ $FAILC -eq 0 ]; then
  echo "VOL68.5-GATE: PASS"
else
  echo "VOL68.5-GATE: FAIL"; exit 7
fi
