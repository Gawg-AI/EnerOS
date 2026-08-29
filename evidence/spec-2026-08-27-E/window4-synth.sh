#!/bin/bash
# SPEC-E 续作 Window 4（synth 正例 + 同盘重启续链）：
#   根因修复后（svc-telemetry 栈 64KiB + 磁盘 TSDB 停用=E-DEF-01）多分块
#   TEL_INGEST 在 target 打穿——qemu-test 装配 + qemu-test-synth 合成遥测
#   （SOC 0.475↔0.525 振荡），无外部馈入件。
#
#   Run1（fresh 数据盘）：INGEST 落地（telemetry-ingest>0 / TSDB-COMMIT）
#                        + LEARN-APPLY 序列 + AUDIT-CHAIN-DURABLE（fresh）；
#   Run2（同 evidence-dir=同数据盘，自动复用）：LEARN-STATE restored=1
#                        current=Run1 末值（E.3-C3② 对照）。
#
# 用法：window4-synth.sh <out_dir> [run1_s] [run2_s]
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.."; pwd)"; cd "$ROOT"
D=$1; RUN1=${2:-420}; RUN2=${3:-240}
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200

IMG=target/eneros/artifacts/qemu-test/eneros-qemu-test.raw
mkdir -p "$D"
if [ -f "$D/eneros-data.raw" ]; then
  echo "WARNING: $D 已有数据盘——本脚本 Run1 需 fresh"; exit 2
fi

run_window() {  # $1=duration $2=reset-bootctrl("reset"|"keep") $3=run-tag
  local EXTRA=""
  [ "$2" = "reset" ] && EXTRA="--reset-bootctrl"
  cargo +nightly-2026-04-04 xtask run qemu-test \
    --image "$IMG" \
    --vars-template target/evidence46-l2/template.fd --expect PASS \
    --evidence-dir "$D" $EXTRA \
    --ready-marker root-heartbeat --qmp-deadline-secs "$1" >"$D/run$3.log" 2>&1
  local RC=$?; case "$RC" in 124|2) RC=0;; esac
  echo "$RC"
}

RC1=$(run_window "$RUN1" reset 1) || true
cp "$D/qemu-serial.log" "$D/qemu-serial.run1.log"

RC2=$(run_window "$RUN2" reset 2) || true
cp "$D/qemu-serial.log" "$D/qemu-serial.run2.log"

S1="$D/qemu-serial.run1.log"; S2="$D/qemu-serial.run2.log"
{
  echo "=== WINDOW4 RC=($RC1,$RC2) ==="
  echo "--- Run1 (fresh) ---"
  echo "TSDB-DISK DISABLED line:  $(grep -m1 'TSDB-DISK DISABLED' "$S1" || echo NONE)"
  echo "TSDB-COMMIT count:        $(grep -c 'TSDB-COMMIT' "$S1" || true)"
  echo "ingest Transport(must 0): $(grep -c 'ingest transport\|INGEST eipc=Transport' "$S1" || true)"
  echo "CONTROL-CHAIN rounds:     $(grep -c 'CONTROL-CHAIN: round=' "$S1" || true)"
  echo "last round line:          $(grep 'CONTROL-CHAIN: round=' "$S1" | tail -1 || echo none)"
  echo "PANIC lines (must 0):     $(grep -c 'PANIC' "$S1" || true)"
  echo "AUDIT-CHAIN-DURABLE:      $(grep -m1 'AUDIT-CHAIN-DURABLE' "$S1" || echo NONE)"
  echo "LEARN-APPLY count:        $(grep -c 'LEARN-APPLY' "$S1" || true)"
  echo "LEARN-APPLY first/last:   $(grep 'LEARN-APPLY' "$S1" | head -1) | $(grep 'LEARN-APPLY' "$S1" | tail -1)"
  echo "LEARN-REJECT-CAP count:   $(grep -c 'LEARN-REJECT-CAP' "$S1" || true)"
  echo "LEARN-STATE (factory):    $(grep -m1 'LEARN-STATE' "$S1" || echo NONE)"
  echo "--- Run2 (reuse disk) ---"
  echo "LEARN-STATE (restored):   $(grep -m1 'LEARN-STATE' "$S2" || echo NONE)"
  echo "LEARN-APPLY count:        $(grep -c 'LEARN-APPLY' "$S2" || true)"
  echo "CONTROL-CHAIN rounds:     $(grep -c 'CONTROL-CHAIN: round=' "$S2" || true)"
  echo "PANIC lines (must 0):     $(grep -c 'PANIC' "$S2" || true)"
} | tee "$D/timeline.txt"
exit 0
