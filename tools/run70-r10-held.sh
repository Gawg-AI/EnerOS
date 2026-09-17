#!/bin/bash
# ============================================================
# CORE-70-R10 专属门（多资产多时段执行/部分失败/重启续接实弹）
#
# 2026-09-17 R10 生产持有入口（ADR-0054）接入后建立：此前 vol89 的
# assets=2 探针只证明请求到达，生产 run_typed 不持有未来步（DEFERRED）
# ——本门以真实逐意图证据取代。
#
# 装配：qemu-test + qemu-test-synth（明文 1883 peer v2 轨 + synth OBS，
# vol89 同款）+ slave61.py（5502，双资产遥测/控制寄存器在位）。
#
#   段A 健康多时段窗（fresh 盘）：
#     A1 peer 收到 v2 请求 assets=2 steps=4；
#     A2 目标 PLAN-HELD ≥1（多命令计划入持有存储）；
#     A3 HELD-BATCH state=Completed ≥1（整批完成）；
#     A4a HELD-COMMAND state=Applied ≥2；
#     A4b 传输级读回验证 ≥2（写后同址 FC03 读回同值——map_receipt
#        对新鲜应用不上浮 readback（对账专属），读回以传输面取证）；
#     A5 modbus transcript：0x200 与 0x201 双资产均有 FC06 WRITE，
#        且写时刻跨 ≥10s（多时段分步发放，非一次倾泻）；
#     A6 零 PANIC / 零分配失败。
#   段B 重启续接窗（同数据盘脏断电）：
#     B0 kill 前提：PLAN-HELD ≥1 且 ≥1 HELD-COMMAND state=Applied；
#     B1 重启窗 HELD-STORE-OPEN status=OK（双槽区打开）；
#     B2 未发步取消：重启窗 HELD-COMMAND state=Blocked ≥1；
#     B3 已确认前缀保留：kill 前 Applied 的 intent 前缀在重启窗
#        仍 state=Applied（attempts 不变），不得出现 Executing（重复
#        发送）；
#     B4 重启窗写归因：FC06 ≤ 新投递数（dispatch=Committed；peer 已死
#        ⇒ 无新远程计划——本地 System1 单命令提案合法，无主写=疑补发）；
#     B5 零 PANIC。
#   段C 部分失败窗（slave --reject-write-addr 0x201）：
#     C1 PLAN-HELD ≥1；
#     C2 transcript：0x200 有 WRITE（资产1健康）∧ 0x201 有
#        WRITE-FI reject-write-addr（资产2被拒）；
#     C3 HELD-COMMAND：≥1 Applied ∧ ≥1 非完成终态
#        （Failed/Blocked/ManualIntervention）——部分失败不吞健康资产；
#     C4 零 PANIC。
#
# 判据器：词形 grep（vol89 同式）；G1 面沿 run70_gate_judge 自检。
# 用法：tools/run70-r10-held.sh（段长可外覆：A_DL=480 B_DL=480 C_DL=480）
# 退出：FAIL=1 → R10-HELD-GATE: FAIL
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$(dirname "$0")/gate-env.sh"
EVROOT="$ROOT/evidence/70/70.15/r10-held"
RUN="$ROOT/target/evidence70-r10-held"
A_DL="${A_DL:-480}"
B_DL="${B_DL:-480}"
C_DL="${C_DL:-480}"
mkdir -p "$EVROOT" "$RUN"
FAIL=0
# 提前退出（ready 未达等）也必须恢复被改写的 mqtt 配置（tracked 文件）
trap 'if [ -f "$RUN/protocol-mqtt.toml.bak" ]; then mv -f "$RUN/protocol-mqtt.toml.bak" "$ROOT/configs/protocol-mqtt.toml"; fi' EXIT

python3 "$ROOT/tools/run70_gate_judge.py" --selftest >/dev/null || {
  echo "判据器自检 FAIL"; exit 2; }
gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2

# ---- 段0：构建（qemu-test-synth；mqtt broker 1883 先于 compile 嵌入）------
MQTT_TOML="$ROOT/configs/protocol-mqtt.toml"
cp -f "$MQTT_TOML" "$RUN/protocol-mqtt.toml.bak"
python3 - "$MQTT_TOML" <<'PY'
import sys, re
p = sys.argv[1]
s = open(p).read()
s = re.sub(r'broker\s*=\s*"10\.0\.2\.2:\d+"', 'broker = "10.0.2.2:1883"', s)
open(p, "w").write(s)
PY
TEST_ROOT_KEY_HEX="1f58396696deec97a0090fa612eb5c4784f0b114693696916c01e9cff8ebc5c7ec0c18caf02a7478c73e54bf13a06fb11624fd1a233ab5e41217aa7bb3803e04"
gate_cleanup_env
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
  --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" >"$RUN/compile.log" 2>&1 || {
  echo "CONFIG-COMPILE FAIL"; tail -5 "$RUN/compile.log"; exit 2; }
gate_relink_cached "$RUN/sel4.log" "sel4-eipc,qemu-test-synth" || { echo "SEL4-RELINK FAIL"; exit 3; }
gate_image_raw_cached qemu-test "$RUN/image.log" "sel4-eipc,qemu-test-synth"
IMG="$ROOT/target/eneros/artifacts/qemu-test/eneros-qemu-test.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

PEER_CFG="$ROOT/evidence/70/70.8/planner-peer.toml"
[ -f "$PEER_CFG" ] || PEER_CFG="$ROOT/crates/ai/planner-peer/planner-peer.toml"

start_broker_slave() {
  # transcript/broker 记录必须**本段新写**（跨运行残留的旧条目会把判据
  # 过滤器炸在缺字段旧行上——A5 ts 提取实测咬过；vol1 门同款清理先例）。
  rm -f "$1/broker-transcript.jsonl" "$1/modbus.transcript.jsonl"
  python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
    --transcript "$1/broker-transcript.jsonl" >"$1/broker.log" 2>&1 &
  BBRK=$!
  sleep 1
  (ss -tln 2>/dev/null | grep -q ":1883 ") || { echo "BROKER-NOT-LISTENING"; exit 4; }
  shift
  python3 "$ROOT/evidence/61/modbus_slave61.py" \
    --transcript "$1/modbus.transcript.jsonl" "${@:2}" &
  BSLV=$!
  sleep 1
}

start_peer() {  # $1=dir（日志）；peer-seq-state 全门共享（seq 单调）
  stdbuf -oL -eL cargo run --quiet --bin eneros-planner-peer -- \
    --broker 127.0.0.1:1883 --config "$PEER_CFG" \
    --state "$RUN/peer-seq-state.json" >>"$1/peer.log" 2>&1 &
  PPEER=$!
  sleep 5
}

start_qemu() {  # $1=dir $2=DL
  rm -f "$1/qemu-serial.log"
  cargo xtask run qemu-test \
    --image "$IMG" --vars-template "$TPL" \
    --expect PASS --evidence-dir "$1" \
    --data-disk "$1/eneros-data.raw" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker root-heartbeat --qmp-deadline-secs "$2" >/dev/null 2>&1 &
  BQEMU=$!
}

wait_ready() {  # $1=dir $2=超时秒
  for _ in $(seq 1 "$(( $2 / 3 ))"); do
    grep -q "PRODUCT-READY: true" "$1/qemu-serial.log" 2>/dev/null && return 0
    sleep 3
  done
  return 1
}

kill_window() {
  kill -9 $(pgrep -f "qemu-syste[m]") 2>/dev/null || true
  sleep 3
  kill $BQEMU $PPEER 2>/dev/null || true
  sleep 1
  # 早退路径（ready 未达 exit 6）也经此清理编队——否则残留 broker 占
  # 1883 会咬后续门（vol89 实测 PORT 仍被占）。
  kill $BBRK $BSLV 2>/dev/null || true
  kill -9 $(pgrep -f "mqtt_telemetry_broke[r]") 2>/dev/null || true
  kill -9 $(pgrep -f "modbus_slave6[1]") 2>/dev/null || true
  sleep 1
}

# ============ 段A：健康多时段窗 =============================================
echo "=== 段A：健康多时段窗（${A_DL}s）==="
DA="$RUN/win-a"; mkdir -p "$DA"
truncate -s 64M "$DA/eneros-data.raw"
start_broker_slave "$DA" "$DA"
start_peer "$DA"
start_qemu "$DA" "$A_DL"
wait_ready "$DA" 240 || { echo "段A PRODUCT-READY 未达"; kill_window; exit 6; }
# 等整批完成（Completed）或窗尾
for _ in $(seq 1 $((A_DL / 10))); do
  grep -q "HELD-BATCH .*state=Completed" "$DA/qemu-serial.log" 2>/dev/null && break
  sleep 10
done
kill_window
kill $BBRK $BSLV 2>/dev/null || true
sleep 1
SA="$DA/qemu-serial.log"
TA="$DA/modbus.transcript.jsonl"
PA="$DA/peer.log"

n=$(grep -ac "PEER-REQ-V2 .* assets=2 steps=4" "$PA" 2>/dev/null || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][A1] v2 请求 assets=2 steps=4（×$n）" || { echo "[✗][A1] v2 多资产请求缺失"; FAIL=1; }
n=$(grep -ac "PLAN-HELD" "$SA" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][A2] PLAN-HELD（×$n）" || { echo "[✗][A2] 零计划持有"; FAIL=1; }
n=$(grep -ac "HELD-BATCH .*state=Completed" "$SA" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][A3] HELD-BATCH Completed（×$n）" || { echo "[✗][A3] 整批未完成"; FAIL=1; }
n=$(grep -ac "HELD-COMMAND .*state=Applied" "$SA" || true); n=${n:-0}
[ "$n" -ge 2 ] && echo "[✓][A4a] 命令 Applied（×$n）" || { echo "[✗][A4a] Applied 仅 ×$n（需 ≥2）"; FAIL=1; }
# A4b：传输级读回验证——写后同址读回同值（map_receipt 对新鲜应用
# 不上浮 readback（对账路径专属），读回事实以传输面取证更硬）。
RBV=$(python3 - "$TA" <<'PY' 2>/dev/null || echo "ERR"
import json, sys
def events(path):
    try:
        fh = open(path)
    except FileNotFoundError:
        return []
    for l in fh:
        try:
            yield json.loads(l)
        except Exception:
            continue
writes, reads = {}, {}
for e in events(sys.argv[1]):
    if e.get("func") != "fc03" or "addr" not in e:
        if e.get("event") == "WRITE":
            writes.setdefault(e["addr"], []).append(e.get("value"))
    elif e.get("event") == "READ":
        reads.setdefault(e["addr"], []).append(e.get("value"))
verified = 0
for addr, wvs in writes.items():
    rv = reads.get(addr, [])
    for w in wvs:
        if w in rv:
            verified += 1
            rv.remove(w)
print(verified)
PY
)
[ "${RBV:-0}" -ge 2 ] && echo "[✓][A4b] 写后同址读回验证（×$RBV）" || { echo "[✗][A4b] 读回验证仅 ×${RBV:-0}（需 ≥2）"; FAIL=1; }

# A5：双资产写入 + 多时段时刻分布（跨度取整秒——bash 算术无浮点）。
W200=$(python3 - "$TA" <<'PY' 2>/dev/null || echo "0 0"
import json, sys
ts = [json.loads(l)["ts"] for l in open(sys.argv[1])
      if json.loads(l).get("event") == "WRITE" and json.loads(l).get("addr") == "0x200"]
print(len(ts), int(max(ts) - min(ts)) if ts else 0)
PY
)
W201=$(python3 - "$TA" <<'PY' 2>/dev/null || echo "0 0"
import json, sys
ts = [json.loads(l)["ts"] for l in open(sys.argv[1])
      if json.loads(l).get("event") == "WRITE" and json.loads(l).get("addr") == "0x201"]
print(len(ts), int(max(ts) - min(ts)) if ts else 0)
PY
)
N200=$(echo "$W200" | awk '{print $1}'); T200=$(echo "$W200" | awk '{print $2}')
N201=$(echo "$W201" | awk '{print $1}'); T201=$(echo "$W201" | awk '{print $2}')
SPREAD=$(( T200 > T201 ? T200 : T201 ))
if [ "${N200:-0}" -ge 1 ] && [ "${N201:-0}" -ge 1 ]; then
  echo "[✓][A5a] 双资产写入（0x200×$N200 / 0x201×$N201）"
else
  echo "[✗][A5a] 双资产写入缺失（0x200×${N200:-0} / 0x201×${N201:-0}）"; FAIL=1
fi
if [ "${SPREAD:-0}" -ge 10 ]; then
  echo "[✓][A5b] 多时段发放（写时刻跨度 ${SPREAD}s ≥10s）"
else
  echo "[✗][A5b] 写时刻跨度 ${SPREAD:-0}s <10s（非分步发放）"; FAIL=1
fi
PANICS=$(grep -ac "PANIC\|alloc.*failed\|ALLOC-FAIL" "$SA" || true); PANICS=${PANICS:-0}
[ "$PANICS" -eq 0 ] && echo "[✓][A6] 零 PANIC/零分配失败" || { echo "[✗][A6] PANIC=$PANICS"; FAIL=1; }

# ============ 段B：重启续接窗（脏断电）======================================
echo "=== 段B：重启续接窗 ==="
DB="$RUN/win-b"; mkdir -p "$DB"
truncate -s 64M "$DB/eneros-data.raw"
start_broker_slave "$DB" "$DB"
start_peer "$DB"
start_qemu "$DB" "$B_DL"
wait_ready "$DB" 240 || { echo "段B 首启 PRODUCT-READY 未达"; kill_window; exit 6; }
# 等 kill 前提：PLAN-HELD 出现即入定时（accept 内联首驱：步 0 当即
# Applied；步 1+ 按步长 15s 递次生效——+12s 切保证 ≥2 步未发）。
BOK=0
for _ in $(seq 1 $((B_DL / 3))); do
  if grep -q "PLAN-HELD" "$DB/qemu-serial.log" 2>/dev/null; then
    BOK=1; break
  fi
  sleep 3
done
[ "$BOK" -eq 1 ] && sleep 12
echo "[段B] kill -9 QEMU（脏断电）"
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 3
kill $BQEMU 2>/dev/null || true
sleep 1
# pre-cut 拷贝在 harness 排空串口尾之后（kill -9 → 管道 EOF → 排空）
cp -f "$DB/qemu-serial.log" "$DB/pre-cut-serial.log"
cp -f "$DB/modbus.transcript.jsonl" "$DB/pre-cut-transcript.jsonl" 2>/dev/null || true
# peer 不再重启：重启窗内零新远程计划——B4 的写零判据才可归因
kill $PPEER 2>/dev/null || true
sleep 2

# 二次启动：同数据盘（复用，绝不 fresh）
start_qemu "$DB" "$B_DL"
wait_ready "$DB" 240 || { echo "段B 二启 PRODUCT-READY 未达"; kill_window; exit 6; }
sleep 90   # 恢复 tick 数轮（取消/对账/保留打印）
kill_window
kill $BBRK $BSLV 2>/dev/null || true
sleep 1

S1="$DB/pre-cut-serial.log"
S2="$DB/qemu-serial.log"
T1="$DB/pre-cut-transcript.jsonl"
T2="$DB/modbus.transcript.jsonl"

n=$(grep -ac "PLAN-HELD" "$S1" || true); n=${n:-0}
m=$(grep -ac "HELD-COMMAND .*state=Applied" "$S1" || true); m=${m:-0}
[ "$n" -ge 1 ] && [ "$m" -ge 1 ] && echo "[✓][B0] kill 前持有+已确认（PLAN-HELD ×$n / Applied ×$m）" \
  || { echo "[✗][B0] kill 前提不满足（PLAN-HELD ×$n / Applied ×$m）"; FAIL=1; }

n=$(grep -ac "HELD-STORE-OPEN status=OK" "$S2" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][B1] 重启 HELD-STORE-OPEN OK（×$n）" || { echo "[✗][B1] 持有存储未打开"; FAIL=1; }
n=$(grep -ac "HELD-COMMAND .*state=Blocked" "$S2" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][B2] 未发步取消 Blocked（×$n）" || { echo "[✗][B2] 零未发步取消"; FAIL=1; }

# B3：kill 前 Applied 的 intent 前缀，重启窗仍 Applied 且零 Executing
python3 - "$S1" "$S2" >"$RUN/b3-check.txt" <<'PY'
import re, sys
pre, post = open(sys.argv[1]).read(), open(sys.argv[2]).read()
ids = set(re.findall(r"HELD-COMMAND intent=\[([0-9a-fx, ]+)\] state=Applied", pre))
ok, bad = 0, []
for i in ids:
    pat = re.compile(r"HELD-COMMAND intent=\[" + re.escape(i) + r"\] state=(\w+)")
    states = pat.findall(post)
    if not states:
        bad.append((i, "absent")); continue
    if any(s == "Executing" for s in states):
        bad.append((i, "re-executed")); continue
    if any(s == "Applied" for s in states):
        ok += 1
    else:
        bad.append((i, states))
print(ok)
for b in bad:
    print("BAD", b[0], b[1])
PY
B3OK=$(head -1 "$RUN/b3-check.txt")
B3BAD=$(grep -c "^BAD" "$RUN/b3-check.txt" || true); B3BAD=${B3BAD:-0}
[ "$B3OK" -ge 1 ] && [ "$B3BAD" -eq 0 ] && echo "[✓][B3] 已确认前缀保留（$B3OK 条，零重复执行）" \
  || { echo "[✗][B3] 前缀保留失败（ok=$B3OK bad=$B3BAD）"; cat "$RUN/b3-check.txt"; FAIL=1; }

# B4：重启窗写归因——每次 FC06 必须可归属到一次**新**投递（本地 System1
# 单命令提案的 dispatch=Committed；peer 已死 ⇒ 无新远程计划）。超出新
# 投递数的写 = 无主写（疑补发）。同计划已确认 intent 的重复执行另由
# B3 的 state=Executing 检查钉死。
N6=$(python3 - "$T1" "$T2" <<'PY' 2>/dev/null || echo "ERR"
import json, sys
def fc06(path):
    try:
        fh = open(path)
    except FileNotFoundError:
        return 0
    n = 0
    for l in fh:
        try:
            e = json.loads(l)
        except Exception:
            continue
        if e.get("func") == "fc06":
            n += 1
    return n
print(fc06(sys.argv[2]) - fc06(sys.argv[1]))
PY
)
NCOMMIT=$(grep -ac "dispatch=Committed" "$S2" || true); NCOMMIT=${NCOMMIT:-0}
if [ "${N6:-99}" -le "$NCOMMIT" ]; then
  echo "[✓][B4] 重启窗 FC06 ×$N6 ≤ 新投递 ×$NCOMMIT（无无主写；对账只读）"
else
  echo "[✗][B4] 重启窗 FC06 ×$N6 > 新投递 ×$NCOMMIT（存在无主写——疑补发）"; FAIL=1
fi
PANICS=$(grep -ac "PANIC\|alloc.*failed\|ALLOC-FAIL" "$S2" || true); PANICS=${PANICS:-0}
[ "$PANICS" -eq 0 ] && echo "[✓][B5] 零 PANIC" || { echo "[✗][B5] PANIC=$PANICS"; FAIL=1; }

# ============ 段C：部分失败窗 ===============================================
echo "=== 段C：部分失败窗（0x201 拒写）==="
DC="$RUN/win-c"; mkdir -p "$DC"
truncate -s 64M "$DC/eneros-data.raw"
start_broker_slave "$DC" "$DC" --reject-write-addr 0x201
start_peer "$DC"
start_qemu "$DC" "$C_DL"
wait_ready "$DC" 240 || { echo "段C PRODUCT-READY 未达"; kill_window; exit 6; }
for _ in $(seq 1 $((C_DL / 10))); do
  grep -q "HELD-COMMAND .*state=\(Failed\|Blocked\|ManualIntervention\)" "$DC/qemu-serial.log" 2>/dev/null && break
  sleep 10
done
kill_window
kill $BBRK $BSLV 2>/dev/null || true
sleep 1
SC="$DC/qemu-serial.log"
TC="$DC/modbus.transcript.jsonl"

n=$(grep -ac "PLAN-HELD" "$SC" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][C1] PLAN-HELD（×$n）" || { echo "[✗][C1] 零计划持有"; FAIL=1; }
NC200=$(grep -ac '"event": "WRITE".*"addr": "0x200"' "$TC" 2>/dev/null || true); NC200=${NC200:-0}
NCFI=$(grep -ac '"event": "WRITE-FI".*"addr": "0x201"' "$TC" 2>/dev/null || true); NCFI=${NCFI:-0}
[ "$NC200" -ge 1 ] && [ "$NCFI" -ge 1 ] && echo "[✓][C2] 部分失败注入生效（0x200 写 ×$NC200 / 0x201 拒 ×$NCFI）" \
  || { echo "[✗][C2] 注入未生效（0x200 ×$NC200 / 0x201 拒 ×$NCFI）"; FAIL=1; }
NAP=$(grep -ac "HELD-COMMAND .*state=Applied" "$SC" || true); NAP=${NAP:-0}
NFT=$(grep -ac "HELD-COMMAND .*state=\(Failed\|Blocked\|ManualIntervention\)" "$SC" || true); NFT=${NFT:-0}
[ "$NAP" -ge 1 ] && [ "$NFT" -ge 1 ] && echo "[✓][C3] 混合终态（Applied ×$NAP / 非完成 ×$NFT）" \
  || { echo "[✗][C3] 混合终态缺失（Applied ×$NAP / 非完成 ×$NFT）"; FAIL=1; }
PANICS=$(grep -ac "PANIC\|alloc.*failed\|ALLOC-FAIL" "$SC" || true); PANICS=${PANICS:-0}
[ "$PANICS" -eq 0 ] && echo "[✓][C4] 零 PANIC" || { echo "[✗][C4] PANIC=$PANICS"; FAIL=1; }

# ---- 收尾：恢复 mqtt 配置 + 归档 -------------------------------------------
if [ -f "$RUN/protocol-mqtt.toml.bak" ]; then
  mv -f "$RUN/protocol-mqtt.toml.bak" "$MQTT_TOML"
fi
for d in win-a win-b win-c; do
  [ -f "$RUN/$d/qemu-serial.log" ] && cp -f "$RUN/$d/qemu-serial.log" "$EVROOT/$d.qemu-serial.log"
done
[ -f "$S1" ] && cp -f "$S1" "$EVROOT/win-b-precut.qemu-serial.log"
for d in win-a win-b win-c; do
  [ -f "$RUN/$d/modbus.transcript.jsonl" ] && cp -f "$RUN/$d/modbus.transcript.jsonl" "$EVROOT/$d.modbus-transcript.jsonl"
done
[ -f "$DA/peer.log" ] && cp -f "$DA/peer.log" "$EVROOT/peer-a.log"
[ -f "$DB/peer.log" ] && cp -f "$DB/peer.log" "$EVROOT/peer-b.log"
[ -f "$DC/peer.log" ] && cp -f "$DC/peer.log" "$EVROOT/peer-c.log"

if [ "$FAIL" -eq 0 ]; then
  echo "=== R10-HELD-GATE: PASS ==="
else
  echo "=== R10-HELD-GATE: FAIL ==="
fi
exit "$FAIL"
