#!/bin/bash
# SPEC-E Window 2：同数据盘重启——E.2 C2（审计链跨重启续链）+ E.3 C3②
# （学习参数快照恢复：LEARN-STATE restored=1 current 须与 W1 末次 LEARN-APPLY 逐字节一致）
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.."; pwd)"; cd "$ROOT"
D=target/evidence-e/window1          # 复用 W1 evidence dir = 同一数据盘
D2=target/evidence-e/window2; mkdir -p "$D2"
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200

# 重启前基准：W1 末次学习值（供 transcript 逐字节对照）。
grep -E 'LEARN-APPLY|LEARN-THRESHOLDS' "$D/qemu-serial.log" | tail -3 > "$D2/w1-final-learn.txt" || true
grep -E 'AUDIT-CHAIN appended' "$D/qemu-serial.log" | tail -1 > "$D2/w1-final-audit.txt" || true

python3 evidence/47/mqtt_broker47.py --transcript "$D2/broker.jsonl" &
B=$!
sleep 2
python3 evidence/spec-2026-08-27-E/peer_brain_e.py --policy fence \
  --transcript "$D2/peer.jsonl" &
P_PEER=$!
python3 evidence/47/scada_write.py --delay 40 --retries 4 --interval 20 \
  --payload "SET 1 10 60.0" > "$D2/scada.out" 2>&1 & P_SCADA=$!

cargo +nightly-2026-04-04 xtask run qemu-test \
  --image target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  --vars-template target/evidence46-l2/template.fd --expect PASS \
  --evidence-dir "$D" --net-mode present --net-user --net-protocols present --reset-bootctrl \
  --ready-marker root-heartbeat --qmp-deadline-secs 600 >"$D2/run.log" 2>&1 &
QPID=$!
wait $QPID; RC=$?; case "$RC" in 124|2) RC=0;; esac  # 到点强杀=窗口完成（证据=判定行）
kill "$B" "$P_PEER" "$P_SCADA" 2>/dev/null

SERIAL="$D/qemu-serial.log"
{
  echo "=== WINDOW2 RC=$RC ==="
  echo "AUDIT-CHAIN-DURABLE verdict: $(grep -o 'AUDIT-CHAIN-DURABLE [a-z-]*.*' "$SERIAL" 2>/dev/null | head -1)"
  echo "LEARN-STATE restored line:   $(grep 'LEARN-STATE' "$SERIAL" 2>/dev/null | head -1)"
  echo "post-restart LEARN-APPLY:    $(grep -c 'LEARN-APPLY' "$SERIAL" 2>/dev/null || echo 0)"
} | tee "$D2/timeline.txt"
exit "$RC"
