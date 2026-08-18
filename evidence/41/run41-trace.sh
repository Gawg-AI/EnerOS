#!/usr/bin/env bash
# 蓝图 41 virtio-blk 排障：带 QEMU trace 的复现轮（观察数据盘 virtio-mmio
# 寄存器序列 / queue notify / blk 处理是否到达）。
# 用法：bash evidence/41/run41-trace.sh [seconds]
set -u
cd "$(dirname "$0")/../.."
SECS="${1:-240}"
IMG="target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
DIR="target/eneros/evidence/41-qemu-trace"
mkdir -p "$DIR"
export PATH="/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin:$PATH"

python3 evidence/40/reset-bootctrl.py "$IMG"
rm -f "$DIR/eneros-data.raw"
truncate -s 16777216 "$DIR/eneros-data.raw"
cp target/eneros/evidence/zero-vars-39.fd "$DIR/AAVMF_VARS.run.fd"

cat > "$DIR/trace-events.txt" <<'EOF'
virtio_mmio_write_offset
virtio_mmio_guest_page
virtio_mmio_queue_write
virtio_queue_notify
virtio_blk_handle_read
virtio_blk_handle_write
virtio_blk_submit_multireq
virtio_blk_req_complete
virtio_blk_rw_complete
EOF

qemu-system-aarch64 -machine virt,gic-version=2,acpi=off -smp 4 -cpu cortex-a57 -m 1G \
  -boot order=c -nographic -no-reboot \
  -trace events="$DIR/trace-events.txt",file="$DIR/virtio-trace.log" \
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
echo "--- serial 关键标记 ---"
grep -aE "STORAGE-HANDOVER|svc-storage|STORAGE-PERSIST|PRODUCT-READY|spawn FAILED|PANIC" "$DIR/qemu-serial.log" | head -15
echo "--- trace 尾部（最后 40 行）---"
tail -40 "$DIR/virtio-trace.log"
