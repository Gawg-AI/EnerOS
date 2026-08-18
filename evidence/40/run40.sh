#!/usr/bin/env bash
# 蓝图 40 QEMU 证据轮：reset bootctrl → boot → 摘取控制闭环服务标记。
# 用法：bash evidence/40/run40.sh [seconds]
set -u
cd "$(dirname "$0")/../.."
SECS="${1:-200}"
IMG="target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
DIR="target/eneros/evidence/40-qemu"
export PATH="/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin:$PATH"

python3 evidence/40/reset-bootctrl.py "$IMG"
rm -f "$DIR/eneros-data.raw"
truncate -s 268435456 "$DIR/eneros-data.raw"
cp target/eneros/evidence/zero-vars-39.fd "$DIR/AAVMF_VARS.run.fd"

qemu-system-aarch64 -machine virt,gic-version=2,acpi=off -smp 4 -cpu cortex-a57 -m 1G \
  -boot order=c -nographic -no-reboot \
  -drive if=pflash,format=raw,readonly=on,file=/usr/share/AAVMF/AAVMF_CODE.secboot.fd \
  -drive if=pflash,format=raw,file="$DIR/AAVMF_VARS.run.fd" \
  -drive if=none,format=raw,file="$IMG",id=enerosdisk \
  -device virtio-blk-device,bus=virtio-mmio-bus.0,drive=enerosdisk \
  -object rng-random,filename=/dev/urandom,id=enerosrng \
  -device virtio-rng-device,bus=virtio-mmio-bus.3,rng=enerosrng \
  -drive if=none,format=raw,cache=none,file="$DIR/eneros-data.raw",id=enerosdata \
  -device virtio-blk-device,bus=virtio-mmio-bus.2,drive=enerosdata \
  > "$DIR/qemu-serial.log" 2>&1 &
QPID=$!
sleep "$SECS"
kill "$QPID" 2>/dev/null
sleep 2
echo "READY-SIGNALED=$(grep -ac 'READY-SIGNALED' "$DIR/qemu-serial.log")"
grep -aE "PRODUCT-READY|spawn FAILED|PANIC|OOM|FAULT-INFO: svc-device" "$DIR/qemu-serial.log" | head -8
