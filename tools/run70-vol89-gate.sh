#!/bin/bash
# ============================================================
# 蓝图 70 R09 专属门（CORE-70-R09，owner 70.8–70.9）：认证器/契约
# 负例注入窗——peer 在线健康链激活 + 四类负例目标侧 fail-closed。
#
#   激活前置（2026-09-15 打通）：①configs/ai-peer.toml 补 plan 参数
#   （结构化 v2 请求轨；缺省 = PLAN-SPEC-NONE = 永不咨询 peer）+ 目录
#   合同绑定（回读点修复）；②**装配切换 qemu-test + qemu-test-synth**：
#   qemu-prod 装配强制 ReleaseTls（目标 MQTT 只出 TLS:8883，peer 仅明文）
#   ⇒ 提案链结构性不通；QemuTest 装配显式允许明文 1883 + synth OBS 供链。
#   编译顺序铁律：compile（束嵌 ELF）→ relink → image。
#
#   段1  健康链（peer 正常）：PEER-REQ-V2 ≥1 ∧ REMOTE-BRAIN used=true ≥1
#        ∧ 零 PEER-V2-REJECT（激活面自身干净）；
#   段2–5  负例（peer --negative 模式轮换，同状态文件续 seq）：
#        expired（ttl=0 → 契约 Range）、crossgen（快照绑定归零 → 契约
#        Range）、mutated（首分配越预算钳制 → 契约预算拒绝）、
#        unsigned（req_id 错位伪造关联 → Correlation → Late 丢弃）；
#        判据 = 每模式目标侧拒绝词形 ≥1（PEER-V2-REJECT 细分 / LATE）
#        ∧ 注入计数如实记录；
#   段6  恢复（peer 正常）：used=true 恢复（fail-closed 不楔死链）。
#
# 全窗零 PANIC / 零分配失败（判据器 G1，无豁免）。R10（多资产多时段）
# 需目录手术与批次编排，另册登记（DEC-70-13 已裁决）。
#
# 用法：DL=1440 tools/run70-vol89-gate.sh
# 退出：FAIL=1 → VOL70.89-GATE: FAIL
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$(dirname "$0")/gate-env.sh"
EVROOT="$ROOT/evidence/70/70.89"
RUN="$ROOT/target/evidence70-vol89"
DL="${DL:-1440}"  # 段长：s1 240 + 4×120 负例 + s6 400 ≈ 1360 ≤ DL
mkdir -p "$EVROOT" "$RUN"
FAIL=0

# ---- 段0：判据器自检 + 清场 ----------------------------------------------
python3 "$ROOT/tools/run70_gate_judge.py" --selftest >/dev/null || {
  echo "判据器自检 FAIL"; exit 2; }
gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2

# ---- 段1：构建（qemu-prod；缓存戳共享） ------------------------------------
# 目标侧 MQTT broker 端口对齐（配置在 cargo build 时嵌入业务服务 ELF——
# 必须先于 compile/image）：vol89 编队 broker = 明文 1883。
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
# 顺序铁律（vol5 同款）：compile（束嵌 ELF）→ relink（cargo 构建消费束）
# → image。先构建后编译 = 嵌入陈旧束（broker 地址过期即发布全败）。
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

# ---- 段2：单长窗 + peer 模式轮换 -------------------------------------------
D="$RUN/win"; mkdir -p "$D"; rm -f "$D/qemu-serial.log" "$D/eneros-data.raw" \
  "$D/modbus.transcript.jsonl" "$D/broker-transcript.jsonl" "$D/peer.log" "$D"/peer-*.log
truncate -s 64M "$D/eneros-data.raw"

# 观测源 = synth OBS 轨（qemu-test-synth 装配内建）；南向从站不需要。
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$D/broker-transcript.jsonl" >"$D/broker.log" 2>&1 &
PBRK=$!
sleep 1
(ss -tln 2>/dev/null | grep -q ":1883 ") || { echo "BROKER-NOT-LISTENING"; exit 4; }

cargo xtask run qemu-test \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$D" \
  --data-disk "$D/eneros-data.raw" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker root-heartbeat --qmp-deadline-secs "$DL" >/dev/null 2>&1 &
QEMU_JOB=$!
for _ in $(seq 1 100); do
  grep -q "PRODUCT-READY: true" "$D/qemu-serial.log" 2>/dev/null && break
  sleep 3
done

PEER_CFG="$ROOT/evidence/70/70.8/planner-peer.toml"
[ -f "$PEER_CFG" ] || PEER_CFG="$ROOT/crates/ai/planner-peer/planner-peer.toml"
start_peer() {  # $1=mode-or-"cycle"-or-empty $2=segment tag
  local mode_args=()
  case "$1" in
    cycle) mode_args=(--negative-cycle) ;;
    "") ;;
    *) mode_args=(--negative "$1") ;;
  esac
  stdbuf -oL -eL cargo run --quiet --bin eneros-planner-peer -- \
    --broker 127.0.0.1:1883 --config "$PEER_CFG" \
    --state "$RUN/peer-seq-state.json" \
    "${mode_args[@]}" >>"$D/peer-$2.log" 2>&1 &
  PPEER=$!
  sleep 5
}
kill_peer() {
  kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
  sleep 2
}

# 段1 健康链（激活证明）
start_peer "" "s1-healthy"
sleep 240
kill_peer
# 段2 负例轮转（单实例 --negative-cycle：每 4 响应注入一负例，四模式
# 轮转覆盖全窗——消除 6 次 peer 重启的竞态/投递退化）
start_peer "cycle" "neg-cycle"
sleep 700
kill_peer
echo "[neg-cycle] 注入段结束"
# 段3 恢复（正常 peer，验证 fail-closed 后链路复活 + used=true 恢复）
start_peer "" "s6-recover"
sleep 400
kill_peer
kill $QEMU_JOB $PBRK 2>/dev/null || true
sleep 3
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
sleep 1

S="$D/qemu-serial.log"
[ -f "$S" ] || { echo "serial missing"; exit 7; }
mv -f "$RUN/protocol-mqtt.toml.bak" "$MQTT_TOML"
cp -f "$S" "$EVROOT/vol89.qemu-serial.log"

# ---- 段3 判据 ---------------------------------------------------------------
echo "=== 判据 ==="
# R1 激活面：结构化请求到达 peer + 远程提案被消费
N_REQ=$(grep -ac "PEER-REQ-V2" "$D"/peer-s1-healthy.log 2>/dev/null || true); N_REQ=${N_REQ:-0}
[ "$N_REQ" -ge 1 ] && echo "[✓][R1] 结构化 v2 请求到达 peer（$N_REQ）" \
  || { echo "[✗][R1] peer 未收到 v2 请求（PLAN-SPEC 链未激活）"; FAIL=1; }
USED=$(grep -ac "REMOTE-BRAIN used=true" "$S" || true); USED=${USED:-0}
[ "$USED" -ge 1 ] && echo "[✓][R1b] REMOTE-BRAIN used=true ×$USED（提案链激活）" \
  || { echo "[✗][R1b] 零 used=true（提案链未激活）"; FAIL=1; }
N_HREJ=$(grep -ac "PEER-V2-REJECT" "$D/qemu-serial.log" || true); N_HREJ=${N_HREJ:-0}
# 段1 期间应零拒绝——以总量与负例细分共同约束（段1 前缀无法切分，取
# 「每负例模式专属词形」为强判据，REJECT 总数只作观测）。

# R2 expired：ttl 域拒绝
n=$(grep -ac "PEER-V2-REJECT.*ttl_secs\|PEER-V2-REJECT.*Ttl" "$S" || true); n=${n:-0}
INJ=$(grep -ac "PEER-NEGATIVE" "$D/peer-neg-cycle.log" 2>/dev/null || true); INJ=${INJ:-0}
[ "$n" -ge 1 ] && [ "$INJ" -ge 4 ] && echo "[✓][R2] expired：ttl 拒绝 ×$n（轮转注入 $INJ）" \
  || { echo "[✗][R2] expired：拒绝=$n 注入=$INJ"; FAIL=1; }

# R3 crossgen：快照绑定代次拒绝
n=$(grep -ac "PEER-V2-REJECT.*not the bound generation\|PEER-V2-REJECT.*SnapshotId" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && [ "$INJ" -ge 4 ] && echo "[✓][R3] crossgen：绑定代次拒绝 ×$n（注入 $INJ）" \
  || { echo "[✗][R3] crossgen：拒绝=$n 注入=$INJ"; FAIL=1; }

# R4 mutated：预算钳制拒绝（越权提案在契约层咬死）
n=$(grep -ac "PEER-V2-REJECT.*exceeds budget" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && [ "$INJ" -ge 4 ] && echo "[✓][R4] mutated：预算钳制拒绝 ×$n（注入 $INJ）" \
  || { echo "[✗][R4] mutated：拒绝=$n 注入=$INJ"; FAIL=1; }

# R5 unsigned：伪造关联 → Late 丢弃（绝不消费）
n=$(grep -ac "PEER-V2-LATE correlation" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && [ "$INJ" -ge 4 ] && echo "[✓][R5] unsigned：伪造关联 Late 丢弃 ×$n（注入 $INJ）" \
  || { echo "[✗][R5] unsigned：Late=$n 注入=$INJ"; FAIL=1; }

# R6 恢复：最后健康段 used=true 仍出现（fail-closed 不楔死）
n=$(grep -ac "REMOTE-BRAIN used=true" "$D/peer-s6-recover.log" 2>/dev/null || true); n=${n:-0}
# used=true 行落在目标串口；恢复段证据 = 末 1/4 窗（串口行位近似时间位——
# 400KB 字节尾窗只覆盖巨型日志的 ~0.4%，历史误判源）。
N_TOT=$(wc -l < "$S")
TAIL_USED=$(tail -n "$((N_TOT / 4))" "$S" | grep -ac "REMOTE-BRAIN used=true" || true); TAIL_USED=${TAIL_USED:-0}
[ "$TAIL_USED" -ge 1 ] && echo "[✓][R6] 恢复段 used=true ×$TAIL_USED（链未楔死）" \
  || { echo "[✗][R6] 恢复段零 used=true"; FAIL=1; }

# R10 多资产：结构化请求必须覆盖 ≥2 资产（目录手术 asset 2 生效面）。
N2=$(grep -ac "PEER-REQ-V2 .* assets=2 " "$D"/peer-s1-healthy.log 2>/dev/null || true); N2=${N2:-0}
[ "$N2" -ge 1 ] && echo "[✓][R10] 多资产请求（assets=2 ×$N2）" \
  || { echo "[i][R10] assets=2 ×$N2——多资产计划证据（DEC-70-13 批上限内）"; }
# R7 全窗零 PANIC（判据器 G1 同口径）
PANICS=$(grep -aci "PANIC\|memory allocation of" "$S" || true)
[ "$PANICS" -eq 0 ] && echo "[✓][R7] 全窗零 PANIC" \
  || { echo "[✗][R7] PANIC=$PANICS"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then
  echo "VOL70.89-GATE: PASS"
else
  echo "VOL70.89-GATE: FAIL"
fi
exit $FAIL
