#!/usr/bin/env bash
# SPEC-SERIES-2026-08-27/C · QEMU 证据窗口驱动（WP-C.1/C.2）。
#
# 用法：run_window.sh <out_dir> <media_raw> [duration_s]
#   out_dir   窗口产物目录（自动建立）
#   media_raw 底盘镜像（eneros-qemu-prod.raw；脚本内做窗口私有拷贝）
#   duration  QEMU 硬超时秒（默认 420）
#
# 结构复刻 B 卷 run_window.sh（fresh VARS+anchor 配对+数据盘 16MiB fresh）
# —— 数据盘每次窗口全新 ⇒ TSDB 区段首挂走 format 分支，串口应出现
#    TSDB-DISK-OPEN（打开行）；负例向 broker 发 history-query point=99
#    （新库无该点位）⇒ mgmt_response 期待 Failed "history:no-such-point:99"
#    （C3 负例：明确错误帧，非空成功、非挂死）。
#
# 判定 grep 口径（verify.sh 单独留证）：见同目录 verify.sh。
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

# broker（宿主侧；guest slirp 直连 1883）：transcript 全程记录请求/响应帧。
python3 "$ROOT/evidence/spec-2026-08-27-B/mqtt_broker_b2.py" \
  --port 1883 --transcript "$OUT/broker.jsonl" \
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

# 等 mgmt/broker 面 ready 标记（svc-mgmt 上线即历史链可问）。
for i in $(seq 1 120); do
  grep -q 'management RPC up' "$OUT/qemu-serial.log" 2>/dev/null && break
  sleep 5
done
sleep 20   # 留出其余服务稳定窗口

# 负例①：未知点位 → 明确错误帧。
python3 "$ROOT/evidence/spec-2026-08-27-C/history_query_client.py" \
  --port 1883 --point 99 --last-n 16 --request-id 7001 \
  --transcript "$OUT/history.transcript.jsonl" | tee -a "$OUT/qemu-history.out"

wait $QPID; RC=$?
kill $BROKER_PID 2>/dev/null || true
set -e
date +%s > "$OUT/host-end.epoch"
echo "WINDOW dir=$OUT media=$(basename "$MEDIA") qemu_rc=$RC dur=${DUR}s" \
  | tee "$OUT/WINDOW.txt"
exit 0
