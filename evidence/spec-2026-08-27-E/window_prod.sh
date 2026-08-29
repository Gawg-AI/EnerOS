#!/usr/bin/env bash
# SPEC-E · qemu-prod 窗口驱动（骨架复刻 SPEC-C run_window.sh；新增
# modbus_slave_e 振荡 SOC 馈入 + peer_brain_e 围栏对端 + scada 意图写入）。
#
# 用法：window_prod.sh <out_dir> [duration_s] [--reuse]
#   --reuse：不重建数据盘/VARS（重启续链窗/负例窗用同一目录介质）。
set -uo pipefail
OUT=$1; DUR=${2:-540}; REUSE=${3:-}
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
MEDIA="${WINDOW_MEDIA:-$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw}"

mkdir -p "$OUT"
if [ "$REUSE" != "--reuse" ]; then
  cp "$ROOT/target/evidence-w59/c45/eneros-data.raw" "$OUT/eneros-data.raw"
  cp "$ROOT/target/evidence-w59/c45/AAVMF_VARS.run.fd" "$OUT/AAVMF_VARS.run.fd"
  cp "$MEDIA" "$OUT/disk.raw"
  python3 "$ROOT/tools/reset-bootctrl.py" "$OUT/disk.raw"
  python3 "$ROOT/evidence/spec-2026-08-27-B/anchor_repair.py" \
    "$OUT/AAVMF_VARS.run.fd" "$OUT/disk.raw"
fi

# 宿主侧伴生进程：broker（mqtt）+ 振荡从站（采集馈入）+ 围栏对端（E.1）。
python3 "$ROOT/evidence/47/mqtt_broker47.py" --transcript "$OUT/broker.jsonl" &
BROKER_PID=$!
sleep 1
python3 "$ROOT/evidence/spec-2026-08-27-E/modbus_slave_e.py" \
  --transcript "$OUT/slave.jsonl" 2> "$OUT/slave.stderr.txt" &
SLAVE_PID=$!
python3 "$ROOT/evidence/spec-2026-08-27-E/peer_brain_e.py" --policy e1-cycle \
  --transcript "$OUT/peer.jsonl" & PEER_PID=$!
sleep 1
# E 续作：SCADA 意图时序可参（prod 编队启动 ~5-10 分钟时，意图须覆盖
# 后段窗口；缺省维持 47 卷原值）。
python3 "$ROOT/evidence/47/scada_write.py" --delay "${SCADA_DELAY:-60}" --retries "${SCADA_RETRIES:-4}" --interval "${SCADA_INTERVAL:-25}" \
  --payload "SET 1 10 50.0" > "$OUT/scada.out" 2>&1 & SCADA_PID=$!

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
wait $QPID; RC=$?
kill $BROKER_PID $SLAVE_PID $PEER_PID $SCADA_PID 2>/dev/null || true
set -e
date +%s > "$OUT/host-end.epoch"

S="$OUT/qemu-serial.log"
{
  echo "=== WINDOW dir=$OUT rc=$RC dur=${DUR}s reuse=$REUSE ==="
  echo "AUDIT-CHAIN-DURABLE:     $(grep -m1 'AUDIT-CHAIN-DURABLE' "$S" || echo NONE)"
  echo "AUDIT-VERIFY-FAIL:       $(grep -m1 'AUDIT-VERIFY-FAIL' "$S" || echo none)"
  echo "AUDIT-CHAIN appended:    $(grep -o 'AUDIT-CHAIN appended=[0-9]* dropped=[0-9]*' "$S" | tail -1 || echo none)"
  echo "LEARN-STATE:             $(grep -m1 'LEARN-STATE' "$S" || echo NONE)"
  echo "LEARN-THRESHOLDS:        $(grep -m1 'LEARN-THRESHOLDS' "$S" || echo none)"
  echo "LEARN-APPLY count:       $(grep -c 'LEARN-APPLY' "$S" || true)"
  echo "LEARN-REJECT-CAP count:  $(grep -c 'LEARN-REJECT-CAP' "$S" || true)"
  echo "REMOTE-BRAIN used=true:  $(grep -c 'used=true' "$S" || true)"
  echo "PEER-MSG verdict=Bad:    $(grep -c 'verdict=Bad' "$S" || true)"
  echo "CONTROL-CHAIN rounds:    $(grep -c 'CONTROL-CHAIN: round=' "$S" || true)"
} | tee "$OUT/timeline.txt"
exit 0
