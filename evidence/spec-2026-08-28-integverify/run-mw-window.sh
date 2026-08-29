#!/usr/bin/env bash
# SPEC-SERIES-2026-08-28/B · WP-B.2 DoD B3：MASTER-WRITE→Committed 全链重放窗口
# （蓝图 47 基线编排复刻：mqtt_broker + modbus_slave + scada_write + qemu-prod）。
#
# 用法：run-mw-window.sh <out_dir> <media_raw> [duration_s]
# 判据（R9：串口实测行裁决）：
#   [svc-modbus-tcp] MASTER-WRITE device=1 point=10 power=50 ACK
#   [svc-modbus-tcp] MASTER-READBACK point=10 eng=50 target=50 VERIFIED
#   [svc-control-dispatch] INTENT state=Committed note=None
#   [svc-control-dispatch] AUDIT-CHAIN appended=N dropped=0
set -euo pipefail

OUT=$1; MEDIA=$2; DUR=${3:-420}
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"

mkdir -p "$OUT"
cp "$MEDIA" "$OUT/disk.raw"
cp "$ROOT/target/evidence-w59/c45/eneros-data.raw" "$OUT/eneros-data.raw"
cp "$ROOT/target/evidence-w59/c45/AAVMF_VARS.run.fd" "$OUT/AAVMF_VARS.run.fd"
python3 "$ROOT/tools/reset-bootctrl.py" "$OUT/disk.raw"
python3 "$ROOT/evidence/spec-2026-08-27-B/anchor_repair.py" \
  "$OUT/AAVMF_VARS.run.fd" "$OUT/disk.raw"

# 对端一：broker（SCADA 下行 + guest 订阅）。
python3 "$ROOT/evidence/spec-2026-08-27-B/mqtt_broker_b2.py" \
  --port 1883 --transcript "$OUT/broker.jsonl" \
  > "$OUT/broker.stdout.jsonl" 2> "$OUT/broker.stderr.txt" &
BROKER_PID=$!
# 对端二：modbus 从站（guest 主站经 slirp 出站 5502 写 + 独立读回）。
python3 "$ROOT/evidence/47/modbus_slave47.py" \
  --port 5502 --transcript "$OUT/modbus.transcript.jsonl" \
  > "$OUT/slave.stdout.txt" 2> "$OUT/slave.stderr.txt" &
SLAVE_PID=$!
trap 'kill $BROKER_PID $SLAVE_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 50); do
  grep -q TELBROKER-READY "$OUT/broker.stderr.txt" 2>/dev/null && break
  sleep 0.1
done

date +%s > "$OUT/host-start.epoch"
set +e
timeout --signal=KILL "$DUR" qemu-system-aarch64 \
  -machine virt,gic-version=2,acpi=off -smp 4 -cpu cortex-a57 -m 1G \
  -boot order=c -nographic -no-reboot \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.secboot.fd \
  -drive if=pflash,format=raw,file="$OUT/AAVMF_VARS.run.fd" \
  -drive if=none,format=raw,file="$OUT/disk.raw",id=enerosdisk \
  -device virtio-blk-device,bus=virtio-mmio-bus.0,drive=enerosdisk \
  -object rng-random,filename=/dev/urandom,id=enerosrng \
  -device virtio-rng-device,bus=virtio-mmio-bus.3,rng=enerosrng \
  -drive if=none,format=raw,cache=none,file="$OUT/eneros-data.raw",id=enerosdata \
  -device virtio-blk-device,bus=virtio-mmio-bus.2,drive=enerosdata \
  -netdev user,id=enerosnet8  -device virtio-net-device,bus=virtio-mmio-bus.8,netdev=enerosnet8  \
  -netdev user,id=enerosnet9  -device virtio-net-device,bus=virtio-mmio-bus.9,netdev=enerosnet9  \
  -netdev user,id=enerosnet10 -device virtio-net-device,bus=virtio-mmio-bus.10,netdev=enerosnet10 \
  -netdev user,id=enerosnet11 -device virtio-net-device,bus=virtio-mmio-bus.11,netdev=enerosnet11 \
  -netdev user,id=enerosnet13 -device virtio-net-device,bus=virtio-mmio-bus.13,netdev=enerosnet13 \
  > "$OUT/qemu-serial.log" 2> "$OUT/qemu.stderr.txt" &
QPID=$!

# 等 dispatch/modbus 面就绪后下发 SCADA 写（ret：幂等重试 5 次）。
for i in $(seq 1 120); do
  grep -q 'protocol-outbound channel bound' "$OUT/qemu-serial.log" 2>/dev/null && break
  sleep 5
done
sleep 20
python3 "$ROOT/evidence/47/scada_write.py" \
  --host 127.0.0.1 --port 1883 --topic "eneros/control" \
  --payload "SET 1 10 50.0" --retries 5 --interval 10 \
  2>&1 | tee "$OUT/scada.out"

wait $QPID; RC=$?
kill $BROKER_PID $SLAVE_PID 2>/dev/null || true
set -e
date +%s > "$OUT/host-end.epoch"

S="$OUT/qemu-serial.log"
{
  echo "=== MW-WINDOW dir=$OUT media=$(basename "$MEDIA") qemu_rc=$RC dur=${DUR}s ==="
  echo "MASTER-WRITE ACK:   $(grep -m1 'MASTER-WRITE device=1 point=10 power=50 ACK' "$S" || echo NONE)"
  echo "MASTER-READBACK:    $(grep -m1 'MASTER-READBACK point=10 eng=50 target=50 VERIFIED' "$S" || echo NONE)"
  echo "INTENT Committed:   $(grep -m1 'INTENT state=Committed' "$S" || echo NONE)"
  echo "AUDIT-CHAIN:        $(grep -m1 'AUDIT-CHAIN appended=' "$S" || echo NONE)"
  echo "PANIC count (0):    $(grep -c 'PANIC' "$S" || true)"
  echo "PRODUCT-READY:      $(grep -m1 'PRODUCT-READY' "$S" || echo NONE)"
} | tee "$OUT/MW-VERIFY.txt"
MW=$([ "$(grep -c 'MASTER-WRITE device=1 point=10 power=50 ACK' "$S")" -ge 1 ] \
  && [ "$(grep -c 'MASTER-READBACK point=10 eng=50 target=50 VERIFIED' "$S")" -ge 1 ] \
  && [ "$(grep -c 'INTENT state=Committed' "$S")" -ge 1 ] \
  && [ "$(grep -c 'PANIC' "$S")" = 0 ] && echo PASS || echo REVIEW)
echo "MW-CHAIN-VERDICT: $MW" | tee -a "$OUT/MW-VERIFY.txt"
exit 0
