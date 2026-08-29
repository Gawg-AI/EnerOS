#!/usr/bin/env bash
# SPEC-SERIES-2026-08-27/B · QEMU 证据窗口驱动。
#
# 用法：run_window.sh <out_dir> <media_raw> [duration_s] [-- broker 扩展参数]
#   out_dir   窗口产物目录（自动建立）
#   media_raw 底盘镜像（qemu-prod.raw / qemu-qemu-test.raw；脚本内做窗口私有拷贝）
#   duration  QEMU 硬超时秒（默认 300）
#   -- 之后参数原样转发给 mqtt_broker_b2.py（如 --expect-username u --expect-password p）
#
# 结构复刻 evidence-w59/c45 实证形态：pflash(既有已注册 VARS 副本) +
# 盘0 = media 拷贝(bus.0) + 数据盘(bus.2, 16MiB fresh) + 协议槽位网
# （8/9/10/11/13 = user slirp；1/13 对端缺席→已知 BLOCKED 降级噪声，不阻断）。
set -euo pipefail

OUT=$1; MEDIA=$2; DUR=${3:-300}; shift 3 || true
BROKER_ARGS=("$@")
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
C45="$ROOT/target/evidence-w59/c45"
# QEMU 追加参数（如 RTC 缺时负例：QEMU_EXTRA="-rtc base=2010-01-01T00:00:00"）。
QEMU_EXTRA=${QEMU_EXTRA:-}

mkdir -p "$OUT"
cp "$MEDIA" "$OUT/disk.raw"
cp "$C45/eneros-data.raw" "$OUT/eneros-data.raw"
cp "$C45/AAVMF_VARS.run.fd" "$OUT/AAVMF_VARS.run.fd"
# 三态引导测试法：fresh 盘起步——bootctrl 双副本 seq=0，Genesis 由本次
# 引导重新初始化（tools/reset-bootctrl.py 同语义；防 AnchorMismatch）。
python3 "$ROOT/tools/reset-bootctrl.py" "$OUT/disk.raw"
# VARS 内锚与归零记录重配对（64B 等长原地改写；见 anchor_repair.py 头注）。
python3 "$ROOT/evidence/spec-2026-08-27-B/anchor_repair.py" \
  "$OUT/AAVMF_VARS.run.fd" "$OUT/disk.raw"

# 凭据模式标记（透传给 broker；transcript 内自描述）。
python3 "$ROOT/evidence/spec-2026-08-27-B/mqtt_broker_b2.py" \
  --port 1883 --transcript "$OUT/broker.jsonl" "${BROKER_ARGS[@]}" \
  > "$OUT/broker.stdout.jsonl" 2> "$OUT/broker.stderr.txt" &
BROKER_PID=$!
trap 'kill $BROKER_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 50); do
  grep -q TELBROKER-READY "$OUT/broker.stderr.txt" 2>/dev/null && break
  sleep 0.1
done

date +%s > "$OUT/host-start.epoch"
set +e
timeout --signal=KILL "$DUR" qemu-system-aarch64 \
  -machine virt,gic-version=2,acpi=off -smp 4 -cpu cortex-a57 -m 1G \
  -boot order=c -nographic -no-reboot $QEMU_EXTRA \
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
  > "$OUT/qemu-serial.log" 2> "$OUT/qemu.stderr.txt"
QEMU_RC=$?
set -e
date +%s > "$OUT/host-end.epoch"
echo "WINDOW dir=$OUT media=$(basename "$MEDIA") qemu_rc=$QEMU_RC dur=${DUR}s" \
  | tee "$OUT/WINDOW.txt"
