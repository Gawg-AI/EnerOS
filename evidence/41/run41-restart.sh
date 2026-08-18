#!/usr/bin/env bash
# 蓝图 41 重启恢复复验轮（Round B）：保留 round A 的数据盘（不 rm），
# 重新引导后验证 svc-ota LOAD 从介质读回 trial 状态（掉电→重启恢复）。
# 用法：bash evidence/41/run41-restart.sh [seconds]
set -u
cd "$(dirname "$0")/../.."
SECS="${1:-300}"
IMG="target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
DIR="target/eneros/evidence/41-qemu-reverify"
mkdir -p "$DIR"
export PATH="/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin:$PATH"

python3 evidence/40/reset-bootctrl.py "$IMG"
# 注意：不删除数据盘（Round A 已写入 trial 状态；cache=none 掉电语义）。
ls -la "$DIR/eneros-data.raw" 2>/dev/null || { echo "数据盘不存在——先跑 run41.sh"; exit 1; }
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
  > "$DIR/qemu-serial-restart.log" 2>&1 &
QPID=$!
sleep "$SECS"
kill "$QPID" 2>/dev/null
sleep 2
echo "READY-SIGNALED=$(grep -ac 'READY-SIGNALED' "$DIR/qemu-serial-restart.log")"
echo "--- storage/ota 关键标记（重启恢复轮）---"
grep -aE "STORAGE-HANDOVER|SELFCHECK|STORAGE-PERSIST|PRODUCT-READY|blk read|spawn FAILED|PANIC" "$DIR/qemu-serial-restart.log" | head -20
