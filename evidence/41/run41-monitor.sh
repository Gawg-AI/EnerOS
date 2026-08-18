#!/usr/bin/env bash
# 蓝图 41 virtio-blk 排障：QEMU monitor 物理内存 dump 轮——svc-storage
# SELFCHECK 读失败后，dump DMA 池物理区（desc/avail/used）确认内容是否
# 真正落在 QEMU 可见的 RAM 中。
# 用法：bash evidence/41/run41-monitor.sh [seconds]
set -u
cd "$(dirname "$0")/../.."
SECS="${1:-240}"
IMG="target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
DIR="target/eneros/evidence/41-qemu-monitor"
MON_PORT=45455
mkdir -p "$DIR"
export PATH="/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin:$PATH"

python3 evidence/40/reset-bootctrl.py "$IMG"
rm -f "$DIR/eneros-data.raw"
truncate -s 16777216 "$DIR/eneros-data.raw"
cp target/eneros/evidence/zero-vars-39.fd "$DIR/AAVMF_VARS.run.fd"

qemu-system-aarch64 -machine virt,gic-version=2,acpi=off -smp 4 -cpu cortex-a57 -m 1G \
  -boot order=c -nographic -no-reboot \
  -monitor tcp:127.0.0.1:$MON_PORT,server,nowait \
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

# watcher：等待 SELFCHECK 失败标记出现（pool 已写），随后 monitor dump。
python3 - "$DIR/qemu-serial.log" "$MON_PORT" <<'EOF' &
import socket, sys, time

serial_log, port = sys.argv[1], int(sys.argv[2])
deadline = time.time() + 230
seen = 0
while time.time() < deadline:
    try:
        with open(serial_log, "rb") as f:
            data = f.read()
        if b"poll spin=0" in data:
            seen += 1
            if seen >= 2:
                break
    except FileNotFoundError:
        pass
    time.sleep(1)

time.sleep(1)
def monitor(cmd):
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=5)
        s.sendall(cmd.encode() + b"\n")
        time.sleep(0.6)
        s.settimeout(2)
        out = b""
        try:
            while True:
                chunk = s.recv(65536)
                if not chunk:
                    break
                out += chunk
        except socket.timeout:
            pass
        s.close()
        return out.decode(errors="replace")
    except Exception as e:
        return f"<monitor error: {e}>"

print("==== pmemsave pool (during poll) ====")
print(monitor("pmemsave 0x409a5000 0x5000 pool-during-poll.bin"))
print(monitor("pmemsave 0x409a5000 0x5000 pool-during-poll2.bin"))
time.sleep(8)
print(monitor("pmemsave 0x409a5000 0x5000 pool-during-poll3.bin"))
EOF
WATCHER=$!

sleep "$SECS"
kill "$QPID" 2>/dev/null
wait "$WATCHER" 2>/dev/null
sleep 1
echo "--- serial 关键标记 ---"
grep -aE "STORAGE-HANDOVER|SELFCHECK|STORAGE-PERSIST|PRODUCT-READY|blk read" "$DIR/qemu-serial.log" | head -12
