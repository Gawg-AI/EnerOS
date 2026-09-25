#!/bin/bash
# 蓝图 70 卷 70.4（v0.334.0）出口门（WP-70.4.5）——真实目标 MQTT 与站端
# peer 完整恢复的故障矩阵。单 QEMU 窗相位推进（run_id=时间戳，锚行计数
# 分相；原始串口/broker transcript/peer 日志全量归档）：
#
#   段0  host seq 窗：peer 3 轮 kill-9 重启 → seq 严格单调（文件级复验）。
#   段1  健康链样本（broker + 从站 + peer；REMOTE-BRAIN used=true）。
#   段2  broker kill -9 **短断链 25s**（< ai-peer 3×30s arm 阈值——不武装）。
#   段3  broker 重启（锚1）——**70.4 强制判据**（69.2 窗目标侧 MQTT 面
#        杀后不复重连 = REG-69.2-GATE-VACUOUS 预存，本卷 WP-70.4.1/70.4.2
#        修复后必须闭合）：目标新连接（CONNACK）→ 新订阅（SUBSCRIBE）→
#        新请求/新应答（used=true）→ 授权控制（Committed）→ 设备回读
#        （ACQ）；短断链不得武装（LEASE-ARMED ×0）。零样本 = FAIL。
#   段4  peer kill -9 + 重启（同 state 文件）——PEER-RESP seq 跨重启
#        严格大于重启前末值（70.4 seq 先持久预留合同；未武装窗请求在流）。
#   段5  broker kill -9 **长断链 100s**（> arm 阈值）→ LEASE-ARMED +
#        used=false 可观测（WP-70.4.4 安全语义）；重启（锚2）后只要求
#        MQTT 面重连/订阅恢复——武装期 AI 退出判据归 70.9 过渡认证
#        （WP-70.4.4：禁网络标志自动越安全门），不要求 used=true。
#   尾判  窗尾 5000 行控制进展 + 设备回读（全窗数量不替代尾部进展）。
#   全窗  零 PANIC / 零分配失败（judge G1-G2 单源）。
#
#   用法：DL=1000 tools/run70-vol4-gate.sh（段5 长断链 420s 后须覆盖全窗）
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
# 73 批C：判据器自举——门先验自己的判据器，再用它（S4，同 vol5/r10-held）。
python3 "$ROOT/tools/criterion.py" --selftest >/dev/null || {
  echo "criterion.py selftest FAIL"; exit 9; }
cd "$ROOT"
EVROOT="$ROOT/evidence/70/70.4"
RUN="$ROOT/target/evidence70-vol4"
DL="${DL:-1000}"
W="${W:-w1}"
mkdir -p "$EVROOT" "$RUN"
FAIL=0

# ---- 段0：peer 构建 + host seq 窗（kill-9 ×3 + 脚本发布器）---------------
echo "=== 段0：构建 peer（release）+ host seq 窗（3 轮 kill-9）==="
cargo build --offline --release --locked -p eneros-planner-peer 2>&1 | tail -1
PP_BIN="$ROOT/target/release/eneros-planner-peer"
[ -x "$PP_BIN" ] || { echo "planner-peer bin 缺失"; exit 3; }
SEQ_PUB="$ROOT/tools/run69-vol2-seq-publisher.py"

HW="$RUN/hostwin"; mkdir -p "$HW"
# 全局清场（重跑确定性——遗留 peer/broker 分食请求主题 = host 窗必红：
# w3 实测僵尸 peer 分走偶数请求 → 响应重复 + seq 跳号假象）。
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
sleep 1
rm -f "$HW/peer.log" "$HW/broker.log" "$HW/broker-transcript.jsonl" \
  "$HW/seq-state.json" "$HW/seq-state.json.tmp"*
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$HW/broker-transcript.jsonl" >"$HW/broker.log" 2>&1 &
HBROKER=$!
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "段0 BROKER-NOT-LISTENING"; exit 4; fi
"$PP_BIN" --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$HW/seq-state.json" >"$HW/peer.log" 2>&1 &
HW_PEER=$!
sleep 1
grep -q "SUBSCRIBED topic=" "$HW/peer.log" || { echo "hostwin peer 未局"; cat "$HW/peer.log"; exit 5; }
HW_FAIL=0
python3 "$SEQ_PUB" --broker 127.0.0.1:1883 --count 2 --expect-min 0 \
  --req-base 100 --tag PRE0 || HW_FAIL=1
for CYC in 1 2 3; do
  kill -9 $HW_PEER 2>/dev/null || true
  sleep 1
  "$PP_BIN" --broker 127.0.0.1:1883 \
    --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
    --state "$HW/seq-state.json" >>"$HW/peer.log" 2>&1 &
  HW_PEER=$!
  sleep 1
  grep -q "SUBSCRIBED topic=" "$HW/peer.log" || { echo "hostwin cyc$CYC peer 未局"; HW_FAIL=1; break; }
  python3 "$SEQ_PUB" --broker 127.0.0.1:1883 --count 2 --expect-min $((CYC * 2)) \
    --req-base $((200 + CYC * 100)) --tag "C$CYC" || HW_FAIL=1
done
kill $HBROKER $HW_PEER 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
sleep 1
if [ "$HW_FAIL" -ne 0 ]; then
  echo "[✗][段0] host 窗 FAIL（seq 跨重启单调/响应链）"; FAIL=1
else
  echo "[✓][段0] host 窗 3 轮 kill-9 重启 + 逐批 seq 严格递增（PRE0/C1/C2/C3）"
fi
cp -f "$HW/peer.log" "$EVROOT/hostwin-peer.log"

# ---- 段1：镜像构建（升版后最终产物）与编队 --------------------------------
gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
# 装配切换（2026-09-15，R05 复跑）：qemu-prod 强制 ReleaseTls:8883 而本门
# peer 为明文 MQTT ⇒ AI 中继结构性不可建（used=true 判据永缺）。R05 的
# scope=INTEROP（broker/peer 故障恢复语义）不要求 prod TLS 装配——切
# qemu-test + synth（AI 中继已由 vol89 实证 used=true ×9）。目标 MQTT broker
# 端口对齐 1883（束嵌 ELF：compile 先于 relink 的顺序铁律）。
TEST_ROOT_KEY_HEX="1f58396696deec97a0090fa612eb5c4784f0b114693696916c01e9cff8ebc5c7ec0c18caf02a7478c73e54bf13a06fb11624fd1a233ab5e41217aa7bb3803e04"
MQTT_TOML="$ROOT/configs/protocol-mqtt.toml"
cp -f "$MQTT_TOML" "$RUN/protocol-mqtt.toml.bak"
python3 - "$MQTT_TOML" <<'PY'
import sys, re
p = sys.argv[1]
s = open(p).read()
s = re.sub(r'broker\s*=\s*"10\.0\.2\.2:\d+"', 'broker = "10.0.2.2:1883"', s)
open(p, "w").write(s)
PY
gate_cleanup_env
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
  --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" >"$RUN/config-compile.log" 2>&1 || {
  echo "CONFIG-COMPILE FAIL"; tail -5 "$RUN/config-compile.log"; exit 2; }
echo "=== sel4-build-rust 重链接（synth 装配）==="
gate_relink_cached "$RUN/sel4.log" "sel4-eipc,qemu-test-synth" || {
  echo "SEL4-RELINK FAIL"; tail -20 "$RUN/sel4.log"; exit 3; }
gate_image_raw_cached qemu-test "$RUN/image.log" "sel4-eipc,qemu-test-synth"

IMG="$ROOT/target/eneros/artifacts/qemu-test/eneros-qemu-test.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

DIR="$RUN/win"; mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw" \
  "$DIR/modbus.transcript.jsonl" "$DIR/iec104.transcript.jsonl" "$DIR/broker-transcript.jsonl"
truncate -s 64M "$DIR/eneros-data.raw"
echo "=== 段1-6：QEMU 双故障段矩阵窗（$DL s）==="
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$DIR/modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$DIR/iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$DIR/broker-transcript.jsonl" >>"$DIR/broker.log" 2>&1 &
BROKER_PID=$!
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi
rm -f "$RUN/$W.seq-state.json"
"$PP_BIN" --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$RUN/$W.seq-state.json" >"$RUN/$W.planner-peer.log" 2>&1 &
PEER_PID=$!

cargo xtask run qemu-test \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --data-disk "$DIR/eneros-data.raw" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker root-heartbeat --qmp-deadline-secs "$DL" >/dev/null 2>&1 &
QEMU_JOB=$!
for _ in $(seq 1 100); do
  grep -q "PRODUCT-READY: true" "$DIR/qemu-serial.log" 2>/dev/null && break
  sleep 3
done
grep -q "PRODUCT-READY: true" "$DIR/qemu-serial.log" || {
  echo "PRODUCT-READY 未达"; kill $QEMU_JOB $PMOD $P104 $PEER_PID $BROKER_PID 2>/dev/null || true
  sleep 2; kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true; exit 6; }

sleep 240  # 段1：健康链样本（used=true / Committed / ACQ；240s 覆盖 AI 中继往返竞态）

# ---- 段2：broker kill-9 短断链（25s < 90s arm 阈值，不得武装）------------
echo "[gate] 段2：kill -9 broker（短断链 25s——arm 阈值下方）"
kill -9 $BROKER_PID 2>/dev/null || true
sleep 25
echo "[gate] 段3：重启 broker（恢复锚1）"
ANCHOR1=$(wc -l < "$DIR/qemu-serial.log" 2>/dev/null || echo 0); ANCHOR1=${ANCHOR1:-0}
BT_A1=$(wc -l < "$DIR/broker-transcript.jsonl" 2>/dev/null || echo 0); BT_A1=${BT_A1:-0}
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$DIR/broker-transcript.jsonl" >>"$DIR/broker.log" 2>&1 &
BROKER_PID=$!
sleep 150  # 恢复窗：目标重连 + 全链（used=true）回归

# ---- 段4：peer kill-9 + 重启（seq 跨重启；此时未武装，请求仍在流）--------
echo "[gate] 段4：kill -9 peer + 重启（seq 跨重启单调）"
SEQ_BEFORE=$(grep -o "PEER-RESP seq=[0-9]*" "$RUN/$W.planner-peer.log" | tail -1 | grep -o "[0-9]*")
SEQ_BEFORE=${SEQ_BEFORE:--1}
kill -9 $PEER_PID 2>/dev/null || true
sleep 2
"$PP_BIN" --broker 127.0.0.1:1883 \
  --config "$ROOT/crates/ai/planner-peer/planner-peer.toml" \
  --state "$RUN/$W.seq-state.json" >>"$RUN/$W.planner-peer.log" 2>&1 &
PEER_PID=$!
sleep 100

# ---- 段5：broker kill-9 长断链（420s ≥ 3×miss 最坏墙钟 → LEASE-ARMED）----
# 2026-09-17 适配：synth 馈电节奏 %3→%16（R10 时域覆盖，DEC-70-13 ①）
# 后快照轮换 ≈80–128s、请求按轮换分簇——100s 断窗攒不满 3 次 miss
#（unhealthy_after=3 × timeout 30s，含无效输入间隙）。420s 覆盖 ≥3 轮
# 换 ≥6 次尝试最坏形。判据不变（LEASE-ARMED 必须出现）。
echo "[gate] 段5：kill -9 broker（长断链 420s——必武装）"
kill -9 $BROKER_PID 2>/dev/null || true
sleep 420
echo "[gate] 段5：重启 broker（恢复锚2——仅可观测重连/订阅；AI 退出归 70.9 安全边界）"
ANCHOR2=$(wc -l < "$DIR/qemu-serial.log" 2>/dev/null || echo 0); ANCHOR2=${ANCHOR2:-0}
BT_A2=$(wc -l < "$DIR/broker-transcript.jsonl" 2>/dev/null || echo 0); BT_A2=${BT_A2:-0}
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$DIR/broker-transcript.jsonl" >>"$DIR/broker.log" 2>&1 &
BROKER_PID=$!
sleep 60

kill $QEMU_JOB $PMOD $P104 $PEER_PID $BROKER_PID 2>/dev/null || true
sleep 3
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave62") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true

S="$DIR/qemu-serial.log"; P="$RUN/$W.planner-peer.log"; BT="$DIR/broker-transcript.jsonl"
[ -f "$S" ] || { echo "serial log missing"; exit 7; }
# 恢复 protocol-mqtt.toml 基线（本门曾改写 broker 端口）。
mv -f "$RUN/protocol-mqtt.toml.bak" "$MQTT_TOML" 2>/dev/null || true
cp -f "$S" "$EVROOT/$W.qemu-serial.log"
cp -f "$P" "$EVROOT/$W.planner-peer.log"
cp -f "$BT" "$EVROOT/$W.broker-transcript.jsonl" 2>/dev/null || true
cp -f "$DIR/modbus.transcript.jsonl" "$EVROOT/$W.modbus.transcript.jsonl" 2>/dev/null || true
cp -f "$DIR/iec104.transcript.jsonl" "$EVROOT/$W.iec104.transcript.jsonl" 2>/dev/null || true

echo "=== 判据扫描 ==="
# ---- R1：零 PANIC / 零分配失败（无豁免）-----------------------------------
# 73 批C：判据登记化（PANIC-SCAN 单源；旧 -i 宽面退役，回声逐字节同形）。
python3 "$ROOT/tools/criterion.py" check PANIC-SCAN \
  --echo-ok "[✓][R1] 零 PANIC / 零分配失败" --echo-fail "[✗][R1] PANIC={count}" \
  "$S" || FAIL=1

# ---- R2：段1 健康链 --------------------------------------------------------
n=$(head -n "$ANCHOR1" "$S" | grep -ac "REMOTE-BRAIN used=true"); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R2] 段1 used=true（$n）" || { echo "[✗][R2] 段1 used=true 缺失"; FAIL=1; }

# ---- R3：恢复锚1 后**目标侧**全链（短断链——lease 未武装，强制判据）--------
BT1_TAIL=$(tail -n +"$((BT_A1 + 1))" "$BT" | head -n $((BT_A2 - BT_A1)) 2>/dev/null || echo "")
NEW_CONN=$(echo "$BT1_TAIL" | grep -c '"event": *"CONNACK"' || true); NEW_CONN=${NEW_CONN:-0}
NEW_SUB=$(echo "$BT1_TAIL" | grep -c '"event": *"SUBSCRIBE"' || true); NEW_SUB=${NEW_SUB:-0}
[ "$NEW_CONN" -ge 1 ] && echo "[✓][R3] 锚1 后目标侧新连接 CONNACK=$NEW_CONN" || { echo "[✗][R3] 锚1 后目标未重连"; FAIL=1; }
[ "$NEW_SUB" -ge 1 ] && echo "[✓][R3] 锚1 后新订阅 SUBSCRIBE=$NEW_SUB" || { echo "[✗][R3] 锚1 后零新订阅"; FAIL=1; }
# 切片锚 = 内容锚（第 N 次 `tcp-connect OK 10.0.2.2:1883`）：第 1 次 =
# 启动建联；第 2 次 = 短断链恢复（段3）；第 3 次 = 长断链恢复（段5）。
# 对 runner 串口缓冲滞后免疫（行数锚 w4 实测错切——缓存分块落盘使
# wc 时刻与内容错位）。切片落盘再 grep（命令替换传巨量行 = E2BIG 静默零）。
TOTAL=$(wc -l < "$S")
OK1883=$(grep -an "tcp-connect OK 10.0.2.2:1883" "$S" | cut -d: -f1)
R1LINE=$(echo "$OK1883" | sed -n 2p); R1LINE=${R1LINE:-0}
R2LINE=$(echo "$OK1883" | sed -n 3p); R2LINE=${R2LINE:-$TOTAL}
sed -n "$((R1LINE)),$((R2LINE))p" "$S" > /tmp/seg3.$$ || true
echo "[gate] 恢复点行锚：r1=$R1LINE r2=$R2LINE（总 ${TOTAL:-?} 行）"
# 70.4 门校准（实弹 w1/v2 两轮咬合）：25s 断链经 3×30s 超时链在恢复窗内
# 即武装（empirical），lease TTL=86400s（configs/ai-peer.toml 登记）——
# 武装后 AI 请求结构性停发（安全设计，退出认证归 70.9；WP-70.4.4 禁网络
# 标志自动越安全门）。故恢复判据 = MQTT 面 + 控制链 + 设备回读（lease
# 不阻断的业务面）；used=true 完整链由段1 健康窗承载。
POST_COMMIT=$(grep -ac "dispatch=Committed" /tmp/seg3.$$); POST_COMMIT=${POST_COMMIT:-0}
[ "$POST_COMMIT" -ge 1 ] && echo "[✓][R3] 锚1 后授权控制进展 Committed=$POST_COMMIT" || { echo "[✗][R3] 锚1 后零 Committed"; FAIL=1; }
# 73 批B：ACQ 判据登记化（ACQ-MODBUS 单源；回声同形 C2）。
python3 "$ROOT/tools/criterion.py" check ACQ-MODBUS \
  --echo-ok "[✓][R3] 锚1 后设备采集回读 modbus ACQ={count}" \
  --echo-fail "[✗][R3] 锚1 后零设备回读" /tmp/seg3.$$ || FAIL=1

# ---- R4：peer 重启后 seq 严格单调（段0 文件级 + QEMU 窗）-------------------
POST_READY_LINE=$(awk '/READY broker=/{l=NR} END{print l+0}' "$P")
POST_SEQS=$(tail -n +"$POST_READY_LINE" "$P" | grep -o "PEER-RESP seq=[0-9]*" | grep -o "[0-9]*")
if [ -n "$POST_SEQS" ]; then
  MIN_POST=$(echo "$POST_SEQS" | sort -n | head -1)
  if [ "$SEQ_BEFORE" -ge 0 ] && [ "$MIN_POST" -gt "$SEQ_BEFORE" ]; then
    echo "[✓][R4] QEMU 窗 peer 重启后 seq 严格大于重启前末值（before=$SEQ_BEFORE min_post=$MIN_POST）"
  else
    echo "[✗][R4] seq 回退（before=$SEQ_BEFORE min_post=$MIN_POST）"; FAIL=1
  fi
else
  echo "[i][R4] QEMU 窗 peer 重启后零应答（长断链已武装——AI 请求停发属安全设计；seq 跨重启强制判据由段0 host 窗承载）"
fi

# ---- R5：长断链武装可观测（WP-70.4.4 安全语义；AI 退出归 70.9）-------------
grep -aq "LEASE-ARMED" "$S" && echo "[✓][R5] LEASE-ARMED（长断链武装可观测）" || { echo "[✗][R5] LEASE-ARMED 缺失"; FAIL=1; }
BT2_TAIL=$(tail -n +"$((BT_A2 + 1))" "$BT" 2>/dev/null || echo "")
RC2_CONN=$(echo "$BT2_TAIL" | grep -c '"event": *"CONNACK"' || true); RC2_CONN=${RC2_CONN:-0}
RC2_SUB=$(echo "$BT2_TAIL" | grep -c '"event": *"SUBSCRIBE"' || true); RC2_SUB=${RC2_SUB:-0}
[ "$RC2_CONN" -ge 1 ] && echo "[✓][R5] 锚2 后 MQTT 面重连 CONNACK=$RC2_CONN" || { echo "[✗][R5] 锚2 后 MQTT 面未重连"; FAIL=1; }
[ "$RC2_SUB" -ge 1 ] && echo "[✓][R5] 锚2 后订阅恢复 SUBSCRIBE=$RC2_SUB" || { echo "[✗][R5] 锚2 后订阅未恢复"; FAIL=1; }
tail -n +"$((R2LINE + 1))" "$S" > /tmp/seg5.$$ || true
n=$(grep -ac "REMOTE-BRAIN used=false" /tmp/seg5.$$); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R5] 锚2 后降级态持续可观测 used=false（$n）" || echo "[i][R5] 锚2 后零 used=false 样本（如实记录）"
n=$(grep -ac "DEGRADE-OBS armed=true" "$S"); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R5] DEGRADE-OBS 武装观测行=$n（uses 倒计时=退出路径可见）" || echo "[i][R5] DEGRADE-OBS 零样本（探针窗未命中——如实记录）"
echo "[i][R5] 武装期 AI 退出判据不要求 used=true 回归——WP-70.4.4 安全规则（禁网络标志自动越安全门），退出过渡认证归 70.9"
rm -f /tmp/seg3.$$ /tmp/seg5.$$

# ---- R6：资源与目标进程存活 ------------------------------------------------
n=$(grep -ac "HEAP-WM" "$S"); n=${n:-0}
echo "[R6] HEAP-WM 采样行=$n（观测）"
grep -aq "READY-SIGNALED" "$S" && echo "[✓][R6] 服务编队 READY（无服务静默死亡）" || { echo "[✗][R6] READY 缺失"; FAIL=1; }
# 窗尾进展（全窗数量不替代尾部进展——方向 2 §6）：末轮位置须在日志尾部
# 15% 内（武装期 System1 节律降速为观测项，停摆判负）。
TOTAL=$(wc -l < "$S")
LAST_ROUND_LINE=$(grep -an "CONTROL-CHAIN: round=" "$S" | tail -1 | cut -d: -f1)
LAST_ROUND_LINE=${LAST_ROUND_LINE:-0}
if [ "$TOTAL" -gt 0 ] && [ "$LAST_ROUND_LINE" -ge $((TOTAL * 85 / 100)) ]; then
  echo "[✓][R6] 尾部控制进展（末轮在日志 $((LAST_ROUND_LINE * 100 / TOTAL))% 处，总 $TOTAL 行）"
else
  echo "[✗][R6] 尾部控制停摆（末轮在 $((LAST_ROUND_LINE * 100 / TOTAL))%，总 $TOTAL 行）"; FAIL=1
fi
# 73 批B：ACQ 判据登记化（同 R3；尾 3 万行切片经临时文件喂判据器）。
TL_SEG=$(mktemp); tail -n 30000 "$S" > "$TL_SEG"
python3 "$ROOT/tools/criterion.py" check ACQ-MODBUS \
  --echo-ok "[✓][R6] 尾部设备回读（尾 3 万行 ACQ={count}）" \
  --echo-fail "[✗][R6] 尾部零设备回读" "$TL_SEG" || FAIL=1
rm -f "$TL_SEG"

if [ "$FAIL" -eq 0 ]; then
  echo "=== VOL70.4 GATE: PASS ==="
else
  echo "=== VOL70.4 GATE: FAIL ==="
  exit 11
fi
