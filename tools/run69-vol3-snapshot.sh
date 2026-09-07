#!/bin/bash
# 蓝图 69 卷 69.3（v0.328.0）Q2 before/after 快照窗（2026-09-07）：
#   抓 11 个协议 bin 的串口词形快照（descriptor + READY/降级词形），
#   归档 evidence/69/vol69.3/{before,after}/。用法：
#     run69-vol3-snapshot.sh before   # 动代码前
#     run69-vol3-snapshot.sh after    # 改造后
#   环境形状 = 69.1 planner-live 同形（modbus/iec104 从站 + MQTT broker）：
#   裸启动会因 readiness 时序竞态判 goose/sv Degraded（窗#2 实测）、mqtt
#   无 broker → ProtocolInit 降级（窗#3 实测）；mms 无 MMS 对端 →
#   ProtocolInit 降级 = 69.1 参照窗同款词形（5618 行），属环境稳态词形，
#   入快照。词形 token 用 grep -o 字面捕获（抗 [diag] 串口穿插污染，
#   窗#3 实测 iec104 READY 行被 diag 前缀粘连）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PHASE="${1:?before|after}"
EVROOT="$ROOT/evidence/69/vol69.3/$PHASE"
RUN="$ROOT/target/evidence69-vol3/$PHASE"
DL=480
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

DIR="$RUN/win"
mkdir -p "$DIR"
rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"

echo "=== $PHASE 快照窗（$DL s，标准对端编队）==="
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
sleep 60   # 迟发词形余量（iec104 READY 在 69.1 窗 5431 行 ≈ PRODUCT-READY 前）

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

# 词形 token 捕获（grep -o 字面/定界——不受行首穿插污染）。
{
  # 11 × descriptor 词形
  grep -oE "\[protocol-service\] (goose|sv|mms|mqtt|iec104|modbus-tcp|modbus-rtu|dlt645|gbt27930|iec101|iec103) plugin=[^ ]+ role=[^ ]+ interop=[^ ]+" "$S"
  # 5-6 × NET-READY 词形（mqtt 需 broker；mms 预期降级不入此列）
  grep -oE "\[svc-(goose|sv|mqtt|iec104|modbus-tcp)\] NET-TRANSPORT READY capability=[^ ]+" "$S"
  # mms 环境稳态降级词形（无 MMS 对端 = 69.1 参照窗 5618 行同款）
  grep -oF "[svc-mms] plugin start FAILED ProtocolInit —— capability 壳（EXECUTE ST_UNAVAILABLE）" "$S" | head -1
  # RTU 两态词形
  grep -oF "[svc-modbus-rtu] RTU-UNAVAILABLE reason=no-handle —— capability 壳（EXECUTE ST_UNAVAILABLE）" "$S" | head -1
} > "$EVROOT/ready-lines.txt"
sort -o "$EVROOT/ready-lines.txt" "$EVROOT/ready-lines.txt"

N=$(wc -l < "$EVROOT/ready-lines.txt")
echo "[i] 快照 token 数 = $N（判据 = 18：11 descriptor + 5 NET-READY + mms 降级 + RTU）"
[ "$N" -eq 18 ] || { echo "TOKEN-COUNT != 18"; sort -u "$EVROOT/ready-lines.txt" | head -25; exit 8; }
# 逐 token 在场核对（重复 descriptor 行去重后仍须 11 个不同服务）。
for b in goose sv mms mqtt iec104 modbus-tcp modbus-rtu dlt645 gbt27930 iec101 iec103; do
  grep -q "\[protocol-service\] $b plugin=" "$EVROOT/ready-lines.txt" || { echo "MISSING descriptor: $b"; exit 9; }
done
for b in goose sv mqtt iec104 modbus-tcp; do
  grep -q "\[svc-$b\] NET-TRANSPORT READY capability=" "$EVROOT/ready-lines.txt" || { echo "MISSING NET-READY: $b"; exit 9; }
done
grep -q "plugin start FAILED ProtocolInit" "$EVROOT/ready-lines.txt" || { echo "MISSING mms degraded wordform"; exit 9; }
grep -q "RTU-UNAVAILABLE reason=no-handle" "$EVROOT/ready-lines.txt" || { echo "MISSING RTU wordform"; exit 9; }
echo "=== $PHASE 快照 OK → $EVROOT/ready-lines.txt ==="
cat "$EVROOT/ready-lines.txt"
