#!/usr/bin/env bash
# EnerOS seL4 断电矩阵（STO-002 掉电一致性）：
#   Run1: tx=2 prepare 落盘后 kill -9 → Run2 重启应回滚旧值
#   Run2: tx=2 commit 落盘后 kill -9 → Run3 重启应保留新值
# 运行前需 truncate 磁盘；QEMU 必须 cache=none（写直达，断电点确定）。
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
QEMU="${SEL4_QEMU:-$HOME/.cache/xpack-qemu-arm-8.2.6-1/bin/qemu-system-aarch64}"
IMG="$ROOT/third_party/sel4-build/top/images/eneros-root-image-arm-qemu-arm-virt"
DISK="$ROOT/target/sel4-disk.raw"
OUT="$ROOT/target/powercut"
mkdir -p "$OUT"

QEMU_ARGS=(-machine virt,gic-version=3,secure=off -cpu cortex-a57 -smp 4 -m 1024M
  -nographic -monitor none
  -device virtio-rng-device
  -device virtio-blk-device,drive=d0
  -drive "file=$DISK,if=none,id=d0,format=raw,cache=none"
  -kernel "$IMG")

run_wait_marker() { # <log> <marker> <timeout_s>
  local log="$1" marker="$2" tmo="$3"
  timeout "$tmo" "$QEMU" "${QEMU_ARGS[@]}" > "$log" 2>&1 &
  local pid=$!
  for _ in $(seq 1 $((tmo * 10))); do
    grep -q "$marker" "$log" 2>/dev/null && break
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.1
  done
  echo "$pid"
}

echo "== Run1: prepare 断电（tx=2 prepare 落盘后 kill -9）"
rm -f "$DISK"; truncate -s 8M "$DISK"
pid=$(run_wait_marker "$OUT/run1.log" "ENEROS-POWERCUT-PREPARE tx=2" 35)
sleep 0.15
kill -9 "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
grep -q "ENEROS-POWERCUT-PREPARE tx=2" "$OUT/run1.log" \
  && echo "RUN1 prepare-powercut OK" || { echo "RUN1 FAIL"; exit 1; }

echo "== Run2: 重启（回滚验证）+ commit 断电"
pid=$(run_wait_marker "$OUT/run2.log" "ENEROS-POWERCUT-COMMIT" 45)
sleep 0.15
kill -9 "$pid" 2>/dev/null
wait "$pid" 2>/dev/null
grep -q '"test":"tx-rollback-oldvalue","result":"PASS"' "$OUT/run2.log" \
  && echo "RUN2 rollback-oldvalue PASS" || { echo "RUN2 rollback FAIL"; exit 1; }
grep -q "ENEROS-POWERCUT-COMMIT" "$OUT/run2.log" \
  && echo "RUN2 commit-powercut OK" || { echo "RUN2 no commit marker"; exit 1; }

echo "== Run3: 重启（commit 断电后新值保留）+ 全量证据"
timeout 40 "$QEMU" "${QEMU_ARGS[@]}" > "$OUT/run3.log" 2>&1
grep -q '"test":"tx-commit-newvalue","result":"PASS"' "$OUT/run3.log" \
  && echo "RUN3 commit-newvalue PASS" || { echo "RUN3 newvalue FAIL"; exit 1; }
grep -q '"test":"tx-recover","result":"PASS"' "$OUT/run3.log" \
  && echo "RUN3 tx-recover PASS" || { echo "RUN3 recover FAIL"; exit 1; }

echo "== 断电矩阵汇总 =="
grep -oE '"test":"[^"]*","result":"PASS"' "$OUT/run1.log" | sort -u
grep -oE '"test":"[^"]*","result":"PASS"' "$OUT/run2.log" | sort -u
grep -oE '"test":"[^"]*","result":"PASS"' "$OUT/run3.log" | sort -u
echo "POWERCUT-MATRIX: ALL PASS"
