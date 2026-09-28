#!/bin/bash
# ============================================================
# 蓝图 78 卷一专属门（冻结输入保留 + 持有周期实弹：三窗）
#
# 2026-09-29（v0.351.0）建立。host 侧四组红测（t18/t18b/t19/t20/t21/t22/
# t23/t24）+ 目标构建已过；本门补**目标侧**三窗证据（蓝图 §3 WP-78.1.4）。
#
# 夹具事实（三轮实弹标定）：peer 请求 assets=3 steps=4，编译常合并为
# 1 批×3 意图（skipped=9）——计划在准入 tick 内毫秒级整体完成，「Applied
# 前缀 + Pending 后缀」中飞形态不存在（r10_b_witness 前提在此夹具不可
# 达——二轮 B0 实证）。窗设计据此标定：
#
#   段A 健康持有窗（fresh 盘；计划完整收敛后杀）：
#     A1 PLAN-HELD ≥1；A2 SNAP-PIN-HELD ≥1（准入即钉）；A3 HELD-COMMAND
#     Applied ≥1；A4 计量（每 tick 同步 SNAP-PIN-SYNC ≥2 + 快照窗留档）；
#     A5 零 PANIC。
#   段A2 中飞击杀窗（fresh 盘；PLAN-HELD 一出现即杀——0.1s 轮询，落点在
#     持久化突发内、终态前；DONE 抢先则整段重试至多一次）。
#   段B 持有中重启恢复窗（= 蓝图窗1；段A2 中飞盘原样重启、无 peer）：
#     恢复对象 = store 内非终态计划；QEMU 重启后合成时间轴回卷、事件被
#     水印防线正确拒绝（十轮实锤：快照重物化在重启窗永不发生）——恢复
#     链全程消费段A 旧介质工件，确定性成立：
#     B1 HELD-STORE-OPEN OK；B2 旧 blob 存活（SNAP-RECOVERED blobs≥1）；
#     B3 旧事件段存活（EVENT-REPLAY recovered≥1）；B4 装配期重钉 + 非空
#     集同步确认（SNAP-PIN-HELD | SNAP-PIN-SYNC n≥1——被钉=段A2 旧 blob
#     本体，按 ID 取回即「旧工件仍可取」的运行时证明）；
#     B5 恢复只读：未发步取消（state=Blocked ≥1）且零 Executing（无重发）；
#     B6 终态释放（SNAP-PIN-RELEASED ≥1）；B7 零 PANIC。
#   段C 快照卷抹除 → 同步拒绝窗（= 蓝图窗4 fail-closed；段A2 盘第二副本
#     整段清零快照区段，无 peer 重启——store 内计划是唯一在途事实）：
#     C1 SNAP-RECOVERED blobs=0（介质真相——非误判）；C2 装配/准入面拒绝
#     （SNAP-PIN-REJECT blob-missing | SYNC-REJECT | assembly-repin-failed
#     任一）；C3 HELD-RETENTION-FAULT ≥1（新准入/新设备写阻断、重评保持，
#     不靠设备写试探恢复）；C4 零 PANIC。
#
#   窗2（满卷写前退让/失败 → 发布 ID 可取）：合成批距 ~46s × 8 批/快照
#   × 容量 ≈6 份 ⇒ 满卷需 ≈40min 实时，不入门（如实登记证据边界）；
#   host 实弹 t7/t8/t15/t22/t23 已锁同码路径；目标侧与 host 走同一
#   event_face 代码（无 cfg 分叉），容量压测归既有 70.15 soak 登记。
#
# 装配：qemu-test + qemu-test-synth（明文 1883 peer v2 轨 + synth OBS，
# r10 同款）+ slave61.py（5502）。判据 = 词形 grep（criterion.py
# PANIC-SCAN 单源）+ r10_b_witness 复用。
# 用法：tools/run78-vol1-gate.sh（段长可外覆：A_DL/B_DL/C_DL）
# 退出：FAIL=1 → VOL78.1-GATE: FAIL
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$(dirname "$0")/gate-env.sh"
EVROOT="$ROOT/evidence/78/vol1"
RUN="$ROOT/target/evidence78-vol1"
A_DL="${A_DL:-420}"
B_DL="${B_DL:-900}"  # 重启(~2min)+快照重物化(~7min)+计划收敛——600s 实测不够（九轮）
C_DL="${C_DL:-300}"
mkdir -p "$EVROOT" "$RUN"
FAIL=0
# 提前退出也必须恢复被改写的 mqtt 配置（tracked 文件）
trap 'if [ -f "$RUN/protocol-mqtt.toml.bak" ]; then mv -f "$RUN/protocol-mqtt.toml.bak" "$ROOT/configs/protocol-mqtt.toml"; fi' EXIT

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

start_peer() {
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

wait_ports_free() {  # 跨段竞态：杀进程到内核回收端口有延迟（七轮实弹）
  for _ in $(seq 1 20); do
    ss -tln 2>/dev/null | grep -qE ":($1) " || return 0
    sleep 0.5
  done
  return 1
}

kill_window() {
  kill -9 $(pgrep -f "qemu-syste[m]") 2>/dev/null || true
  sleep 3
  kill $BQEMU $PPEER 2>/dev/null || true
  sleep 1
  kill $BBRK $BSLV 2>/dev/null || true
  kill -9 $(pgrep -f "mqtt_telemetry_broke[r]") 2>/dev/null || true
  kill -9 $(pgrep -f "modbus_slave6[1]") 2>/dev/null || true
  sleep 1
}

# ============ 段A：持有中击杀窗（fresh 盘）==================================
echo "=== 段A：持有中击杀窗（${A_DL}s）==="
DA="$RUN/win-a"; mkdir -p "$DA"
rm -f "$DA/eneros-data.raw"
truncate -s 64M "$DA/eneros-data.raw"
start_broker_slave "$DA" "$DA"
start_peer "$DA"
start_qemu "$DA" "$A_DL"
wait_ready "$DA" 240 || { echo "段A PRODUCT-READY 未达"; kill_window; exit 6; }
# 等计划完整收敛（PLAN-HELD ≥1 且 HELD-PLAN-DONE ≥1）——段A 盘 = 终态盘
# （B 窗的恢复对象是介质工件与新计划，不是旧计划的未竟事务）。
for _ in $(seq 1 $((A_DL / 3))); do
  grep -q "PLAN-HELD " "$DA/qemu-serial.log" 2>/dev/null \
    && grep -q "HELD-PLAN-DONE " "$DA/qemu-serial.log" 2>/dev/null && break
  sleep 3
done
sleep 3    # 串口尾排空（含 SNAP-PIN-RELEASED）
kill_window
kill $BBRK $BSLV $PPEER 2>/dev/null || true
sleep 1
SA="$DA/qemu-serial.log"

n=$(grep -ac "PLAN-HELD " "$SA" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][A1] PLAN-HELD ×$n（准入落账）" || { echo "[✗][A1] 无计划准入"; FAIL=1; }
n=$(grep -ac "SNAP-PIN-HELD " "$SA" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][A2] SNAP-PIN-HELD ×$n（冻结输入钉在位）" || { echo "[✗][A2] 无保留钉"; FAIL=1; }
n=$(grep -ac "HELD-COMMAND .* state=Applied" "$SA" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][A3] HELD-COMMAND Applied ×$n（持有期设备写在位）" || { echo "[✗][A3] 无 Applied 步"; FAIL=1; }
NSYNC=$(grep -ac "SNAP-PIN-SYNC n=" "$SA" || true); NSYNC=${NSYNC:-0}
NBOUND=$(grep -ac "SNAP-INPUT-BOUND " "$SA" || true); NBOUND=${NBOUND:-0}
echo "[i][A4] 计量留档：SNAP-PIN-SYNC ×$NSYNC / SNAP-INPUT-BOUND ×$NBOUND / PLAN-HELD ×$n"
[ "$NSYNC" -ge 2 ] && echo "[✓][A4] 每 tick 同步在位（sync ×$NSYNC ≥2：install 首发 + tick 周期）" \
  || { echo "[✗][A4] 周期同步缺失（sync ×$NSYNC <2）"; FAIL=1; }
python3 "$ROOT/tools/criterion.py" check PANIC-SCAN "$SA" \
  --echo-ok "[✓][A5] 零 PANIC" --echo-fail "[✗][A5] PANIC={count}" >/dev/null 2>&1 \
  && echo "[✓][A5] 零 PANIC" || { echo "[✗][A5] PANIC 或判据输入缺失"; FAIL=1; }

# ============ 段A2：中飞击杀窗（fresh 盘；供段B/段C 在途事实）=============
echo "=== 段A2：中飞击杀窗（PLAN-HELD 一出现即杀，0.1s 轮询）==="
a2_ok=0
for attempt in 1 2; do
  DA2="$RUN/win-a2-$attempt"; mkdir -p "$DA2"
  rm -f "$DA2/eneros-data.raw"
  truncate -s 64M "$DA2/eneros-data.raw"
  wait_ports_free "1883|5577" || { echo "段间端口未释放"; exit 4; }
  start_broker_slave "$DA2" "$DA2"
  start_peer "$DA2"
  start_qemu "$DA2" "$A_DL"
  wait_ready "$DA2" 240 || { echo "段A2[$attempt] PRODUCT-READY 未达"; kill_window; exit 6; }
  # PLAN-HELD 一出现即杀（0.1s 轮询）：落点 = 准入持久化突发内（数百块
  # IPC ≈1-3s）、终态前——store 内必有非终态计划（或撕裂态，durable
  # store 生成号+校验裁决）。DONE 抢先（轮询间隙越过终态）则整段重试。
  for _ in $(seq 1 $((A_DL * 10))); do
    grep -q "PLAN-HELD " "$DA2/qemu-serial.log" 2>/dev/null && break
    sleep 0.1
  done
  kill_window
  kill $BBRK $BSLV $PPEER 2>/dev/null || true
  sleep 1
  if ! grep -q "HELD-PLAN-DONE " "$DA2/qemu-serial.log" 2>/dev/null; then
    a2_ok=1
    echo "[✓][A2] 中飞击杀（attempt=$attempt——DONE 未打印，计划非终态在 store）"
    break
  fi
  echo "[i][A2] attempt=$attempt 击杀落在 DONE 后——重试"
done
[ "$a2_ok" -eq 1 ] || echo "[i][A2] 两次尝试均落在 DONE 后（中飞前提退化——段B/段C 判据按 fail-closed 面如实裁决）"
grep -q "PLAN-HELD " "$DA2/qemu-serial.log" 2>/dev/null \
  || { echo "[✗][A2] 未见计划准入（窗内无计划）"; FAIL=1; }
mkdir -p "$RUN/win-b" "$RUN/win-c"
cp -f "$DA2/eneros-data.raw" "$RUN/win-b/eneros-data.raw"
cp -f "$DA2/eneros-data.raw" "$RUN/win-c/eneros-data.raw"

# ============ 段B：持有中重启恢复窗（= 蓝图窗1；段A2 中飞盘）==============
echo "=== 段B：持有中重启恢复窗（${B_DL}s，中飞盘原样重启、无 peer）==="
DB="$RUN/win-b"
wait_ports_free "1883|5577" || { echo "段间端口未释放"; exit 4; }
start_broker_slave "$DB" "$DB"
start_qemu "$DB" "$B_DL"
wait_ready "$DB" 240 || { echo "段B PRODUCT-READY 未达"; kill_window; exit 6; }
sleep 90   # 恢复 tick 数轮（重钉/取消/对账/释放打印）
kill_window
kill $BBRK $BSLV 2>/dev/null || true
sleep 1
S2="$DB/qemu-serial.log"

n=$(grep -ac "HELD-STORE-OPEN status=OK" "$S2" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][B1] 重启窗 HELD-STORE-OPEN OK（×$n）" || { echo "[✗][B1] 持有存储未打开"; FAIL=1; }
n=$(grep -ac "SNAP-RECOVERED blobs=" "$S2" || true); n=${n:-0}
NBLOBS=$(grep -a "SNAP-RECOVERED blobs=" "$S2" | tail -1 | sed 's/.*blobs=\([0-9]*\).*/\1/' || echo 0)
[ "$n" -ge 1 ] && [ "${NBLOBS:-0}" -ge 1 ] \
  && echo "[✓][B2] 旧 blob 存活（SNAP-RECOVERED blobs=$NBLOBS）" \
  || { echo "[✗][B2] 快照卷恢复异常（blobs=${NBLOBS:-?}）"; FAIL=1; }
n=$(grep -ac "EVENT-REPLAY recovered=" "$S2" || true); n=${n:-0}
NREC=$(grep -a "EVENT-REPLAY recovered=" "$S2" | tail -1 | sed 's/.*recovered=\([0-9]*\).*/\1/' || echo 0)
[ "$n" -ge 1 ] && [ "${NREC:-0}" -ge 1 ] \
  && echo "[✓][B3] 旧事件段存活（EVENT-REPLAY recovered=$NREC）" \
  || { echo "[✗][B3] 事件面恢复异常（recovered=${NREC:-?}）"; FAIL=1; }
# B4（十二轮实弹标定的真生产链）：telemetry 惰性开卷（首个 OBS）前，
# 装配重钉/同步 IPC 一律 face-not-open 拒绝 ⇒ FAULT+STALE fail-closed；
# 开卷后重评通道收敛（RECOVERED）且 STALE 不再发生（末条 STALE 早于
# face-open 证据 SNAP-STORED/首条 EVENT-STORE append）。恢复取消（首
# tick）先于开卷（分钟级）⇒ 重钉落空、计划终态释放按 absent-confirm
# 收敛——「故障→重评→收敛」全链实弹。
nrej=$(grep -ac "SNAP-PIN-REJECT reason=face-not-open" "$S2" || true); nrej=${nrej:-0}
nf=$(grep -ac "HELD-RETENTION-FAULT" "$S2" || true); nf=${nf:-0}
nst=$(grep -ac "HELD-SYNC-STALE" "$S2" || true); nst=${nst:-0}
nrc=$(grep -ac "HELD-RETENTION-RECOVERED" "$S2" || true); nrc=${nrc:-0}
LASTSTALE=$(grep -an "HELD-SYNC-STALE" "$S2" | tail -1 | cut -d: -f1); LASTSTALE=${LASTSTALE:-0}
FACEOPEN=$(grep -an "SNAP-STORED kept=\|EVENT-STORE append n=" "$S2" | head -1 | cut -d: -f1); FACEOPEN=${FACEOPEN:-999999999}
if [ "$nrej" -ge 1 ] && [ "$nf" -ge 1 ] && [ "$nst" -ge 1 ] && [ "$nrc" -ge 1 ] \
   && [ "$LASTSTALE" -lt "$FACEOPEN" ]; then
  echo "[✓][B4] 钉/同步 fail-closed（face-not-open ×$nrej → FAULT → 重评 RECOVERED）+ 同步收敛（末 STALE L$LASTSTALE < face-open L$FACEOPEN）"
else
  echo "[✗][B4] 恢复链不完整（rej=$nrej fault=$nf stale=$nst recovered=$nrc 末STALE=L$LASTSTALE faceOpen=L$FACEOPEN）"
  FAIL=1
fi
nb=$(grep -ac "state=Blocked" "$S2" || true); nb=${nb:-0}
ne=$(grep -ac "state=Executing" "$S2" || true); ne=${ne:-0}
[ "$nb" -ge 1 ] && [ "$ne" -eq 0 ] \
  && echo "[✓][B5] 恢复只读（未发步取消 Blocked ×$nb；零 Executing 无重发）" \
  || { echo "[✗][B5] 恢复面异常（Blocked=$nb Executing=$ne）"; FAIL=1; }
n=$(grep -ac "SNAP-PIN-RELEASED \|SNAP-PIN-RELEASE-CONFIRMED-ABSENT" "$S2" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][B6] 释放面收敛（RELEASED|ABSENT-CONFIRM ×$n——重钉未落时 absent 幂等语义实弹，F70-04）" \
  || { echo "[✗][B6] 无释放词形"; FAIL=1; }
python3 "$ROOT/tools/criterion.py" check PANIC-SCAN "$S2" \
  --echo-ok "[✓][B7] 零 PANIC" --echo-fail "[✗][B7] PANIC={count}" >/dev/null 2>&1 \
  && echo "[✓][B7] 零 PANIC" || { echo "[✗][B7] PANIC 或判据输入缺失"; FAIL=1; }

# ============ 段C：快照卷抹除 → 同步拒绝窗（= 蓝图窗4 fail-closed）========
echo "=== 段C：快照卷抹除→同步拒绝窗（${C_DL}s，段A2 盘、无 peer）==="
DC="$RUN/win-c"
wait_ports_free "1883|5577" || { echo "段间端口未释放"; exit 4; }
# 快照区段整段清零（DATA_SNAP_BASE_BLOCK=25224 ×512B，512 块 = 256KiB；
# 常量单源 = ota-ab ab_partition.rs——改动须同步本行，golden 对拍在门尾）。
dd if=/dev/zero of="$DC/eneros-data.raw" bs=512 seek=25224 count=512 conv=notrunc status=none
start_broker_slave "$DC" "$DC"
start_qemu "$DC" "$C_DL"
wait_ready "$DC" 240 || { echo "段C PRODUCT-READY 未达（fault 态就绪性见串口）"; kill_window; exit 6; }
sleep 45   # 装配 + 数轮重评 tick（故障保持打印）
kill_window
kill $BBRK $BSLV 2>/dev/null || true
sleep 1
S3="$DC/qemu-serial.log"

n=$(grep -ac "SNAP-RECOVERED blobs=0" "$S3" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][C1] 介质真相（抹除后 SNAP-RECOVERED blobs=0——非误判）" \
  || { echo "[✗][C1] 抹除未生效或卷恢复异常"; FAIL=1; }
n=$(grep -ac "SNAP-PIN-REJECT reason=blob-missing-or-corrupt" "$S3" || true)
ns=$(grep -ac "SNAP-PIN-SYNC-REJECT" "$S3" || true)
nf=$(grep -ac "HELD-RETENTION-FAULT reason=assembly-repin-failed" "$S3" || true)
[ "$n" -ge 1 ] || [ "$ns" -ge 1 ] || [ "$nf" -ge 1 ] \
  && echo "[✓][C2] 装配/准入面同步拒绝（PIN-REJECT ×$n / SYNC-REJECT ×$ns / repin-failed ×$nf）" \
  || { echo "[✗][C2] 无任何拒绝词形（在途事实缺失？见 A2 标记）"; FAIL=1; }
n=$(grep -ac "HELD-RETENTION-FAULT" "$S3" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][C3] HELD-RETENTION-FAULT（新准入/新设备写阻断；重评保持 ×$n）" \
  || { echo "[✗][C3] 保留故障态未置位"; FAIL=1; }
python3 "$ROOT/tools/criterion.py" check PANIC-SCAN "$S3" \
  --echo-ok "[✓][C4] 零 PANIC" --echo-fail "[✗][C4] PANIC={count}" >/dev/null 2>&1 \
  && echo "[✓][C4] 零 PANIC" || { echo "[✗][C4] PANIC 或判据输入缺失"; FAIL=1; }
# C5（信息行，十三轮实标）：恢复只读取消清空 live 集 ⇒ 故障按重钉面语义
# 清除（保留只拦有存活引用的写面）——blob 缺失对**新准入**的阻断由 C2/C3
# （装配面 repin-failed）+ host t18/t23 承载；RECOVERED 在 C 窗出现与否
# 取决于取消 tick 与 face-open 的竞速，不构成判据。
n=$(grep -ac "HELD-RETENTION-RECOVERED" "$S3" || true); n=${n:-0}
echo "[i][C5] RECOVERED ×$n（存活计划取消后 live 集清空 ⇒ 故障按设计清除；不判红）"

# ---- 门尾：区段常量对拍（dd 偏移与 ota-ab 单源一致——漂移即门红）----
python3 - <<'PY' || { echo "[✗][G1] 快照区段常量漂移（dd 偏移 25224 ≠ ota-ab 单源）"; FAIL=1; }
import re
s = open("crates/security/ota-ab/src/ab_partition.rs").read()
env = {}
for m in re.finditer(r"pub const (\w+): u64 = ([0-9]+);", s):
    env[m.group(1)] = int(m.group(2))
for m in re.finditer(r"pub const (\w+): u64 = (\w+) \+ (\w+);", s):
    env[m.group(1)] = env.get(m.group(2), 0) + env.get(m.group(3), 0)
base = env.get("DATA_SNAP_BASE_BLOCK")
assert base == 25224, f"DATA_SNAP_BASE_BLOCK={base}"
print("[✓][G1] dd 偏移与 ota-ab 单源一致（DATA_SNAP_BASE_BLOCK=25224）")
PY

if [ "$FAIL" -eq 0 ]; then
  echo "VOL78.1-GATE: PASS"
else
  echo "VOL78.1-GATE: FAIL"
fi
exit "$FAIL"
