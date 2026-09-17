#!/bin/bash
# 蓝图 70 卷 70.1（v0.331.0）QEMU 实弹窗——OOM 根因修复验收（WP-70.1.4/5）：
#   压力形 = 69.5 窗同形（modbus/iec104 从站 + MQTT broker，peer 刻意离线
#   → 决策链最高轮速）。69.5 窗在 512 次 agent=OK 处因决策审计 Vec 扩容
#   （1024×112=114688B）PANIC（evidence/69/vol69.5/qemu-serial.log:203189）；
#   2026-09-17 复核撤回降标：TSDB 阻塞修复后 vol2 1200s 有 4230 次
#   Committed，零检查点失败/丢弃；512 并非架构上不可达。恢复 >512 的
#   目标压力覆盖，G5 仍是另一项最小活性判据，两者不能互相替代。
#   段0 另保留生产 64KiB 审计预算的 host 10000 次满容量验证。
#   判据单源 = tools/run70_gate_judge.py（G1–G7，无 alloc.rs 豁免——
#   69.5 的豁免正是 F70-7-02 缺陷）；本脚本先跑其负例自检，再实弹。
#   长稳态窗（≥10000 轮且 ≥2h，P 建议值）预算裁决后以
#   MIN_ROUNDS/DL 环境变量覆写运行，不在本默认窗内凑数。
#   用法：MIN_ROUNDS=520 DL=900 run70-vol1-gate.sh
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/70/70.1"
RUN="$ROOT/target/evidence70-vol1/win"
DL="${DL:-900}"
MIN_ROUNDS="${MIN_ROUNDS:-520}"
mkdir -p "$EVROOT" "$RUN"

# ---- 段0：判据器负例自检（注入 alloc PANIC/尾部死亡/缺日志/旧样本）----
python3 "$ROOT/tools/run70_gate_judge.py" --selftest || {
  echo "判据器自检 FAIL——门不可信"; exit 2; }
# 同一生产实现/预算，显式覆盖旧扩容点及满载拒绝，失败不得进入目标窗。
cargo test -p eneros-dual-brain --features coordinator --lib --locked \
  audit::tests::test_audit_production_budget_survives_10000_decisions -- --exact || exit 2

gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582 2404 1883; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

DIR="$RUN"
mkdir -p "$DIR"
rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
# 对端 transcript 必须**本轮新写**：files 落 EVROOT（持久），旧实现从不
# 清理 ⇒ 三个对端进程本轮启动即死，前一轮的记录仍让 G7 判"非空存活"
# （70.14 复查红测）。清空 = 只有本轮写入才能让 G7 通过。
rm -f "$EVROOT/modbus.transcript.jsonl" "$EVROOT/iec104.transcript.jsonl" \
      "$EVROOT/broker-transcript.jsonl"

echo "=== 70.1 持续控制压力窗（$DL s；peer 离线 = 原失败压力形；门限 $MIN_ROUNDS 轮）==="
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$EVROOT/iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$EVROOT/broker-transcript.jsonl" >"$RUN/broker.log" 2>&1 &
PBRK=$!
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi
# 刻意不启动 planner-peer——与 69.5 失败窗同形（决策链最高轮速压力）。

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
wait_ready || { echo "PRODUCT-READY 未达"; kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null; exit 6; }
sleep "$DL"

kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null || true
sleep 3
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave62") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true

S="$DIR/qemu-serial.log"
[ -f "$S" ] || { echo "serial log missing"; exit 7; }
cp -f "$S" "$EVROOT/qemu-serial.log"
cp -f "$EVROOT/broker-transcript.jsonl" "$EVROOT/broker-transcript.jsonl" 2>/dev/null || true

# ---- 结构化判据裁决（G1–G7 单源；退出码即门结果）----
# 窗长显式声明：G5 的提交下限由"窗长 / 架构决策节奏上界（45s）"推导
# （见 judge 的 CONTROL_CADENCE_MAX_S）。
python3 "$ROOT/tools/run70_gate_judge.py" "$S" "$MIN_ROUNDS" \
  --window-secs "$DL" \
  --transcripts "$EVROOT/modbus.transcript.jsonl" "$EVROOT/iec104.transcript.jsonl"
RC=$?
ROUNDS=$(grep -c "CONTROL-CHAIN: round=" "$S")
OK_N=$(grep -c "agent=OK" "$S")
echo "[summary] rounds=$ROUNDS agent=OK=$OK_N（目标压力需越过512；审计满载另见段0 host；门限=$MIN_ROUNDS 轮）"
if [ "$OK_N" -le 512 ]; then
  echo "[FAIL] target pressure did not cross original 512-decision failure point"
  RC=1
fi
if [ "$RC" -eq 0 ]; then
  echo "=== VOL70.1 QEMU WINDOW: PASS ==="
else
  echo "=== VOL70.1 QEMU WINDOW: FAIL ==="
fi
exit "$RC"
