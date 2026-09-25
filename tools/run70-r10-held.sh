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
#   段A 健康多时段窗（fresh 盘；2026-09-18 三资源 + 守恒重订）：
#     A1 peer 收到 v2 请求 assets=3 steps=4（三资源最低演示形）；
#     A2 PLAN-HELD ≥1 且某计划 PLAN-COMPILED intents+skipped == 12
#        （3 资源 × 4 时段全分配守恒——分辨率合并计入 skipped，不丢账）；
#     A3 该计划 HELD-PLAN-DONE state=Committed（全部批终态收敛——
#        等待条件即此行，非首个批 Completed）；
#     A4a HELD-COMMAND **唯一 intent** state=Applied == 该计划 intents
#        （按 intent id 去重——周期重复打印不计数）；
#     A4b 外部读回**逐计划归属**（recheck RR2：tools/r10_intent_bind.py——
#        HELD-COMMAND 绑定锚 dev/pt/val × 点表 × transcript 序列匹配；
#        通过 = 全部 Applied intent 均获「自己期望的写 + 写后读回」证明，
#        他计划事务不得补数；序列敏感 oracle 保留为组件自检）；
#     A4c HELD-COMMAND readback=Some ≥1（dispatch 独立读回上浮进 Saga
#        ——ADR-0054:15-16 实际回读可查询；None 缺证如实判负）；
#     A5 modbus transcript（**绑定写**口径，recheck RR2）：0x200/0x201/
#        0x202 三资产均有**本计划绑定** FC06 WRITE；时段性判据分两形——
#        非合并计划（skipped=0）绑定写时刻跨 ≥10s（真分步发放）；
#        合并计划（skipped>0，分辨率折叠是编译器合法形态）改**反倾泻**：
#        绑定写总数 ≤ intents（写数超出计划分配 = 无主写），多时段性由
#        A1 重复 v2 steps=4 请求 + A2 的 I+S=12 守恒承载；
#     A6 零 PANIC / 零分配失败；
#     A7 守恒：HELD-PLAN-DONE applied+blocked+failed == intents
#        （每命令终态可对账——执行/跳过/失败/取消全记账）。
#   段B 重启续接窗（同数据盘脏断电）：
#     B0 kill 前提：PLAN-HELD ≥1 且 ≥1 HELD-COMMAND state=Applied；
#     B1 重启窗 HELD-STORE-OPEN status=OK（双槽区打开）；
#     B2 未发步取消：重启窗 HELD-COMMAND state=Blocked ≥1；
#     B3 已确认前缀保留：kill 前 Applied 的 intent 前缀在重启窗
#        仍 state=Applied（attempts 不变），不得出现 Executing（重复
#        发送）；
#     B4 重启窗写归因（RR2 哲学重订）：FC06 ≤ journal 确认执行数
#        （EXEC-JOURNAL applied-confirmed——held settle 与 legacy 单命令
#        两路权威投递 trace；broker QoS1 迟到投递属合法新执行）；
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
# 73 批B：criterion 判据器自举（S4：门先验自己的判据器，再用它）。
python3 "$ROOT/tools/criterion.py" --selftest >/dev/null || {
  echo "criterion.py selftest FAIL"; exit 9; }
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
rm -f "$DA/eneros-data.raw"
truncate -s 64M "$DA/eneros-data.raw"
start_broker_slave "$DA" "$DA"
start_peer "$DA"
start_qemu "$DA" "$A_DL"
wait_ready "$DA" 240 || { echo "段A PRODUCT-READY 未达"; kill_window; exit 6; }
# 等完整计划收敛（HELD-PLAN-DONE state=Committed——全部批终态）或窗尾。
# 首批 Completed ≠ 计划完成（12 分配拆 ≥2 批；旧等待条件会提前杀窗）。
for _ in $(seq 1 $((A_DL / 10))); do
  grep -q "HELD-PLAN-DONE .*state=Committed" "$DA/qemu-serial.log" 2>/dev/null && break
  sleep 10
done
kill_window
kill $BBRK $BSLV 2>/dev/null || true
sleep 1
SA="$DA/qemu-serial.log"
TA="$DA/modbus.transcript.jsonl"
PA="$DA/peer.log"

# —— 段A 判据（A2/A3/A4a/A4c/A7 由守恒对账脚本一次算出）——
# 守恒模型（按 plan id 关联）：PLAN-COMPILED（紧邻 PLAN-HELD 前的编译行）
# intents I + skipped S；HELD-PLAN-DONE applied A + blocked Bk + failed F。
# 通过线：存在计划使 I+S==12 ∧ A+Bk+F==I ∧ state=Committed ∧ A==I。
# F70-05：守恒脚本退出码即判据（0=有满足计划；1/2=无/不可判）——
# 失败说明输出走 stderr（逐计划缺口），不与 stdout 计数混流。
ACCT=$(python3 "$ROOT/tools/r10_plan_accounting.py" "$SA" 2>"$DA/acct-reject.log")
ACCT_RC=$?
[ $ACCT_RC -ne 0 ] && ACCT="ERR"
n=$(grep -ac "PEER-REQ-V2 .* assets=3 steps=4" "$PA" 2>/dev/null || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][A1] v2 请求 assets=3 steps=4（×$n）" || { echo "[✗][A1] 三资源 v2 请求缺失"; FAIL=1; }
if [ "$ACCT" != "ERR" ] && [ "$ACCT" != "NONE" ]; then
  I=$(echo "$ACCT" | awk '{print $1}'); SK=$(echo "$ACCT" | awk '{print $2}')
  A=$(echo "$ACCT" | awk '{print $3}'); BK=$(echo "$ACCT" | awk '{print $4}'); F=$(echo "$ACCT" | awk '{print $5}')
  UNIQ=$(echo "$ACCT" | awk '{print $6}'); RBS=$(echo "$ACCT" | awk '{print $7}')
  # 73 批C：A2/A3/A4a/A4c/A7 是**观测回声**，不是二次判定。守恒谓词
  # （I+S=12 ∧ A+Bk+F=I ∧ Committed ∧ A=I ∧ uniq=I ∧ rbs≥1）唯一执行面是
  # r10_plan_accounting.passes()，其退出码即裁决；本分支只在 select() 通过
  # 后进入，故旧重算臂（`[ "$((I+SK))" -eq 12 ]` 等）结构不可达——谁削弱
  # select() 这里照样全 ✓，"bash 独立判一遍"是假象。逐臂覆盖改由
  # plan_accounting selftest 的 A07 证伪表承担（A4a 的 I≥1 由 uniq==I
  # 臂蕴含，见 A07b）。与 A3 同口径：只回声。
  echo "[✓][A2] 计划守恒 intents=$I skipped=$SK（I+S=12）"
  echo "[✓][A3] HELD-PLAN-DONE Committed（applied=$A blocked=$BK failed=$F）"
  echo "[✓][A4a] 唯一 intent Applied ×$UNIQ ≥ intents=$I"
  echo "[✓][A4c] Applied 带独立 readback=Some（×$RBS 行）"
  echo "[✓][A7] 终态守恒 A+Bk+F == intents"
else
  echo "[✗][A2/A3/A7] 无满足守恒的完整计划（需 intents+skipped=12 ∧ 全 Applied ∧ Committed）：$ACCT"
  FAIL=1; UNIQ=0
fi
# A4b（recheck RR2 重订）：外部读回按**选中计划归属**判定——绑定工具把
# transcript 事务锚到该计划各 intent 期望的 (addr, 值, 时序)，通过 = 全部
# Applied intent 均获「自己期望的写 + 写后读回」的外部证明。旧谓词对
# 整份 transcript 计数后与 UNIQ 比较——另一计划 Q 的事务可给 P 补数
#（Q-only 夹具实测误过门；序列敏感 oracle 保留为组件自检）。
python3 "$ROOT/tools/r10_readback_oracle.py" --selftest >/dev/null || { echo "[✗][A4b] oracle 自检 FAIL"; FAIL=1; }
BIND=$(python3 "$ROOT/tools/r10_intent_bind.py" "$SA" "$TA" 2>"$DA/bind-reject.log"); BIND_RC=$?
[ $BIND_RC -ne 0 ] && BIND="0 0 0
0 0 0 0 0 0"
BPR=$(echo "$BIND" | awk 'NR==1{print $1}'); BTOT=$(echo "$BIND" | awk 'NR==1{print $2}')
[ "$BIND_RC" -eq 0 ] && [ "${BPR:-0}" -ge "${UNIQ:-0}" ] && [ "${UNIQ:-0}" -ge 1 ] \
  && echo "[✓][A4b] 逐计划写后读回 ×$BPR ≥ 唯一 Applied ×$UNIQ（事务绑定到该计划）" \
  || { echo "[✗][A4b] 外部读回归属不足（rc=$BIND_RC proven=${BPR:-0}/total=${BTOT:-0} uniq=${UNIQ:-0}）"; FAIL=1; }

# A5（recheck RR2 重订）：三资产写入 + 多时段时刻分布——按**选中计划的
# 绑定写**计算（绑定工具输出行2；他计划事务不构成本计划的时段证据。
# 跨度取整秒——bash 算术无浮点）。
WTS=$(echo "$BIND" | sed -n '2p'); [ -z "$WTS" ] && WTS="0 0 0 0 0 0"
N200=$(echo "$WTS" | awk '{print $1}'); T200=$(echo "$WTS" | awk '{print $2}')
N201=$(echo "$WTS" | awk '{print $3}'); T201=$(echo "$WTS" | awk '{print $4}')
N202=$(echo "$WTS" | awk '{print $5}'); T202=$(echo "$WTS" | awk '{print $6}')
SPREAD=$(( T200 > T201 ? (T200 > T202 ? T200 : T202) : (T201 > T202 ? T201 : T202) ))
if [ "${N200:-0}" -ge 1 ] && [ "${N201:-0}" -ge 1 ] && [ "${N202:-0}" -ge 1 ]; then
  echo "[✓][A5a] 三资产本计划绑定写入（0x200×$N200 / 0x201×$N201 / 0x202×$N202）"
else
  echo "[✗][A5a] 三资产绑定写入缺失（0x200×${N200:-0} / 0x201×${N201:-0} / 0x202×${N202:-0}）"; FAIL=1
fi
# A5b（recheck RR2 重立法）：时段性判据按计划形态分派——
#   非合并计划（skipped=0）：绑定写时刻跨 ≥10s（真分步发放）；
#   合并计划（skipped>0）：步折叠是编译器合法形态（同值步合并 ⇒ 结构性
#   同刻发放），改**反倾泻**判据——绑定写总数 ≤ intents（超出 = 无主写）。
# 多时段性由 A1（重复 v2 steps=4 请求）+ A2（I+S=12 守恒）承载。
BW=$(echo "$WTS" | awk '{print $1+$3+$5}')
STEP_S=$(python3 -c "import tomllib;print(tomllib.load(open('configs/ai-peer.toml','rb'))['plan_step_ns']//1_000_000_000)" 2>/dev/null || echo 2)
SPREAD_MIN=$(( 3 * STEP_S * 80 / 100 ))  # (步数-1)=3 × step_ns × 80%——步距由 ai-peer 单源
if [ "${SK:-0}" -gt 0 ]; then
  [ "${BW:-0}" -le "${I:-0}" ] \
    && echo "[✓][A5b] 合并计划反倾泻（绑定写 ×$BW ≤ intents=$I；skipped=$SK 步折叠）" \
    || { echo "[✗][A5b] 绑定写 ×${BW:-0} 超过计划分配 ×${I:-0}（无主写/倾泻）"; FAIL=1; }
else
  [ "${SPREAD:-0}" -ge "$SPREAD_MIN" ] \
    && echo "[✓][A5b] 多时段发放（绑定写时刻跨度 ${SPREAD}s ≥ 公式下界 ${SPREAD_MIN}s = 3×step_${STEP_S}s×80%）" \
    || { echo "[✗][A5b] 非合并计划绑定写跨度 ${SPREAD:-0}s < ${SPREAD_MIN}s（非分步发放）"; FAIL=1; }
fi
# 73 批C：A6 判据登记化（PANIC-SCAN 单源）。旧第三词表
# `alloc.*failed|ALLOC-FAIL` 中 ALLOC-FAIL 全仓零 emitter（幻影 token），
# 而真实形 `memory allocation of` 登记册已覆盖——收编后归档重放 0 翻转。
python3 "$ROOT/tools/criterion.py" check PANIC-SCAN \
  --echo-ok "[✓][A6] 零 PANIC/零分配失败" --echo-fail "[✗][A6] PANIC={count}" \
  "$SA" || FAIL=1

# ============ 段B：重启续接窗（脏断电）======================================
echo "=== 段B：重启续接窗 ==="
DB="$RUN/win-b"; mkdir -p "$DB"
rm -f "$DB/eneros-data.raw"
truncate -s 64M "$DB/eneros-data.raw"
start_broker_slave "$DB" "$DB"
start_peer "$DB"
start_qemu "$DB" "$B_DL"
wait_ready "$DB" 240 || { echo "段B 首启 PRODUCT-READY 未达"; kill_window; exit 6; }
# 2026-09-22 冷构建隔离复验：盲切 ready+40s 实测 B0 旧 grep 假绿——
# 首个计划已 DONE（batches=1/intents=3/skipped=9），却仍把旧 PLAN-HELD
# 和 Applied 分别计数；B2/B3 如实红。不能降低 B2/B3 判据或借别的
# 计划补数。按真实未完成计划的 Applied 前缀 + Pending 后缀轮询，
# 串口可能滞后：检测到的 witness 仅作割切触发，kill 后 EOF 排空的
# pre-cut 文件必须由同一工具再次独立核验（晚到 DONE => FAIL）。
B_CUT_WAIT_S="${B_CUT_WAIT_S:-80}"
B_CUT_FOUND=0
for ((j=0; j<B_CUT_WAIT_S*4; j++)); do
  if python3 "$ROOT/tools/r10_b_witness.py" witness "$DB/qemu-serial.log" >/dev/null 2>&1; then
    B_CUT_FOUND=1
    break
  fi
  sleep 0.25
done
if [ "$B_CUT_FOUND" -eq 0 ]; then
  echo "[✗][B-cut] ${B_CUT_WAIT_S}s 内无同计划 Applied+Pending 在途见证（不以盲切伪造覆盖）"
  FAIL=1
fi
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

B0=$(python3 "$ROOT/tools/r10_b_witness.py" witness "$S1" 2>&1); B0_RC=$?
[ "$B0_RC" -eq 0 ] && echo "[✓][B0] $B0" \
  || { echo "[✗][B0] $B0（rc=$B0_RC）"; FAIL=1; }

n=$(grep -ac "HELD-STORE-OPEN status=OK" "$S2" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][B1] 重启 HELD-STORE-OPEN OK（×$n）" || { echo "[✗][B1] 持有存储未打开"; FAIL=1; }
# B2/B3 必须与 B0 同 plan 同 intent 验证（尝试次数也不能漂移）；
# 任何其他计划的 Blocked / Applied 都不可替该计划填空。
BVERIFY=$(python3 "$ROOT/tools/r10_b_witness.py" verify "$S1" "$S2" 2>&1)
BVERIFY_RC=$?
[ "$BVERIFY_RC" -eq 0 ] && { echo "[✓][B2] 同计划未发步 Blocked"; echo "[✓][B3] $BVERIFY"; } \
  || { echo "[✗][B2/B3] $BVERIFY（rc=$BVERIFY_RC）"; FAIL=1; }

# B4：重启窗写归因（recheck RR2 同哲学——journal 投递 trace 而非词形计数）——
# 每次新 FC06 必须可归属到一次**journal 确认的执行**（`EXEC-JOURNAL
# applied-confirmed（独立回读匹配→Committed）`——held settle 与 legacy
# System1 单命令两路都经 journal；恢复前缀只读对账不重落 journal）。旧谓词
# 用 `dispatch=Committed` 行数做上界——那是每轮投影态：broker QoS1 队列把
# 割前 peer 请求投递到重连的 svc-mqtt 时（远程计划迟到仍属合法投递）会
# 误报"无主写"。同计划已确认 intent 的重复执行另由 B3 的 state=Executing
# 检查钉死。边界：被从站拒写的尝试（WRITE-FI）不在 B 窗（无故障注入），
# 属 C 窗判据。
N6=$(python3 - "$T1" "$T2" <<'PY' 2>/dev/null || echo "ERR"
import json, sys
def fc06(path):
    try:
        fh = open(path)
    except FileNotFoundError:
        sys.exit(3)  # 73 批B：缺输入=红（旧 return 0 使 0-0=0 ≤ 0 恒绿）
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
# 73 批B：B4 缺输入=红（T1/T2 由 fc06 exit-3 承载，S2 由判据器
# on_missing_input 承载）；正常路径比较与回声逐字节保持。
if [ "$N6" = "ERR" ]; then
  echo "[✗][B4] 重启窗转写缺失（input missing: pre-cut/modbus transcript）——缺输入=红"; FAIL=1
else
  NDELIV=$(python3 "$ROOT/tools/criterion.py" check R10-B4-DELIVERY --count-only "$S2" 2>/dev/null); NDELIV_RC=$?
  if [ "$NDELIV_RC" -ne 0 ]; then
    echo "[✗][B4] 重启窗 journal 输入缺失（input missing: $S2）——缺输入=红"; FAIL=1
  elif [ "${N6:-99}" -le "${NDELIV:-0}" ]; then
    echo "[✓][B4] 重启窗 FC06 ×$N6 ≤ journal 确认执行 ×$NDELIV（写可归属；对账只读）"
  else
    echo "[✗][B4] 重启窗 FC06 ×${N6:-?} > journal 确认执行 ×${NDELIV:-0}（存在无归属写——疑补发）"; FAIL=1
  fi
fi
# 73 批B：B5 判据登记化（PANIC-SCAN 单源；旧第三词表退役）。
python3 "$ROOT/tools/criterion.py" check PANIC-SCAN \
  --echo-ok "[✓][B5] 零 PANIC" --echo-fail "[✗][B5] PANIC={count}" \
  "$S2" || FAIL=1

# ============ 段C：部分失败窗 ===============================================
echo "=== 段C：部分失败窗（0x201 拒写）==="
DC="$RUN/win-c"; mkdir -p "$DC"
rm -f "$DC/eneros-data.raw"
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
# 73 批C：C4 判据登记化（同 A6；本门 段B/B5 早已走登记册，段A/段C 滞留
# 裸 grep 即"同一门三种口径"）。缺串口 = 红（旧 `grep … || true` 在文件
# 缺失时计 0 而恒绿，与 B4 同类的缺输入假面）。
python3 "$ROOT/tools/criterion.py" check PANIC-SCAN \
  --echo-ok "[✓][C4] 零 PANIC" --echo-fail "[✗][C4] PANIC={count}" \
  "$SC" || FAIL=1

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
