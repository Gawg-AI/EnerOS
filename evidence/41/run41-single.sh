#!/usr/bin/env bash
# 蓝图 41 存储移交 QEMU 复验轮：reset bootctrl → boot → 摘取存储移交
# 与真落盘标记。
# 用法：bash evidence/41/run41.sh [seconds]
set -u
cd "$(dirname "$0")/../.."
SECS="${1:-300}"
IMG="target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
DIR="target/eneros/evidence/41-qemu-single"
mkdir -p "$DIR"
export PATH="/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin:$PATH"

python3 evidence/40/reset-bootctrl.py "$IMG"
rm -f "$DIR/eneros-data.raw"
# 数据盘：16MB（与 qemu harness 默认一致；cache=none 断电语义）。
truncate -s 16777216 "$DIR/eneros-data.raw"
cp target/eneros/evidence/zero-vars-39.fd "$DIR/AAVMF_VARS.run.fd"

qemu-system-aarch64 -machine virt,gic-version=2,acpi=off -accel tcg,thread=single -smp 4 -cpu cortex-a57 -m 1G \
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
echo "--- storage/ota 关键标记 ---"
grep -aE "STORAGE-HANDOVER|svc-storage|svc-ota|PRODUCT-READY|spawn FAILED|PANIC|OOM|FAULT-INFO" "$DIR/qemu-serial.log" | head -25
