#!/bin/bash
# 蓝图 69 卷 69.4（v0.329.0）QEMU 实弹窗（2026-09-07）：
#   Q3 旧 70B 读臂词形退役核对：串口零 SUBMIT/SUBMIT-ERR（退役）；
#      主路 TYPED 词形（仅错误路径打印）不回归新词形漂移。
#   Q4 决策链端到端：CONTROL-CHAIN round 行 dispatch=Committed。
#   Q7 环境稳态：零 CHAIN-KEY-MISSING（与 69.2/69.3 窗同判据）。
#   环境形状 = 69.3 快照窗同形（modbus/iec104 从站 + MQTT broker）。
#   用法：run69-vol4-gate.sh
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/69/vol69.4"
RUN="$ROOT/target/evidence69-vol4/win"
DL=840
mkdir -p "$EVROOT" "$RUN"

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

echo "=== 69.4 实弹窗（$DL s，标准对端编队）==="
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
sleep 120  # 决策链样本余量（CONTROL-CHAIN round 行需多轮样本）

kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null || true
sleep 3
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave62") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true

S="$DIR/qemu-serial.log"
[ -f "$S" ] || { echo "serial log missing"; exit 7; }
cp -f "$S" "$EVROOT/qemu-serial.log"
grep -q "PRODUCT-READY: true" "$S" || { echo "PRODUCT-READY 未 true"; exit 10; }

FAIL=0
# ---- Q4：决策链端到端（CONTROL-CHAIN round 行 → dispatch=Committed）----
ROUNDS=$(grep -c "CONTROL-CHAIN: round=" "$S")
COMMITTED=$(grep -oE "dispatch=Committed" "$S" | wc -l)
echo "[Q4] CONTROL-CHAIN rounds=$ROUNDS dispatch=Committed x$COMMITTED"
[ "$ROUNDS" -ge 1 ] || { echo "[✗][Q4] 无 CONTROL-CHAIN round 行"; FAIL=1; }
[ "$COMMITTED" -ge 1 ] || { echo "[✗][Q4] 无 dispatch=Committed"; FAIL=1; }
# ---- Q3：旧 70B 读臂词形退役（零命中）----
SUBMIT_N=$(grep -c "svc-control-dispatch] SUBMIT" "$S")
TYPED_ERR_N=$(grep -c "svc-control-dispatch] TYPED-ERR" "$S")
echo "[Q3] 退役词形 SUBMIT* x$SUBMIT_N（判据 0）；TYPED-ERR x$TYPED_ERR_N（主路健康时应为 0）"
[ "$SUBMIT_N" -eq 0 ] || { echo "[✗][Q3] 已退役词形 SUBMIT 复现"; FAIL=1; }
[ "$TYPED_ERR_N" -eq 0 ] || { echo "[✗][Q3] TYPED-ERR 非零——主路错误路径回归"; FAIL=1; }
# ---- Q7：环境稳态 ----
CKM=$(grep -c "CHAIN-KEY-MISSING" "$S")
echo "[Q7] CHAIN-KEY-MISSING x$CKM（判据 0）"
[ "$CKM" -eq 0 ] || { echo "[✗][Q7] CHAIN-KEY-MISSING 复现"; FAIL=1; }
# PANIC 零（alloc.rs 已知豁免同 69.2/69.3 窗）
PANIC=$(grep -a "PANIC: panicked at" "$S" | grep -vc "alloc.rs" || true)
echo "[Q7] 非豁免 PANIC x$PANIC（判据 0）"
[ "$PANIC" -eq 0 ] || { echo "[✗][Q7] 非豁免 PANIC"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then echo "=== VOL69.4 QEMU WINDOW: PASS ==="; else echo "=== VOL69.4 QEMU WINDOW: FAIL ==="; exit 11; fi
