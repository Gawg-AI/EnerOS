#!/bin/bash
# SPEC-E Window 1：E.1 围栏对端编排 + E.2 审计链首建 + E.3 学习闭环首窗
# （fresh 数据盘；判据行进 serial：AUDIT-CHAIN-DURABLE fresh / LEARN-STATE /
#   LEARN-APPLY / LEARN-REJECT-CAP / REMOTE-BRAIN used=true / PEER-MSG Bad）
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.."; pwd)"; cd "$ROOT"
D=target/evidence-e/window1; rm -rf "$D"; mkdir -p "$D"
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200

# 防呆：清场残留 broker/对端，等 1883 释放（并发窗口互踩教训）。
pkill -f "mqtt_broker47" 2>/dev/null; sleep 1
for i in $(seq 1 10); do ss -tln 2>/dev/null | grep -q ":1883 " || break; sleep 1; done

python3 evidence/47/mqtt_broker47.py --transcript "$D/broker.jsonl" &
B=$!
sleep 2
# E.1 判据对端：e1-cycle = fence×2 → drift×2 → strings → fence。
python3 evidence/spec-2026-08-27-E/peer_brain_e.py --policy e1-cycle \
  --transcript "$D/peer.jsonl" &
P_PEER=$!
# E.2 审计链喂入：SCADA 周期写意图（每条 intent 经 12 态链 → 审计入链）。
python3 evidence/47/scada_write.py --delay 40 --retries 4 --interval 20 \
  --payload "SET 1 10 50.0" > "$D/scada.out" 2>&1 & P_SCADA=$!

cargo +nightly-2026-04-04 xtask run qemu-test \
  --image target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  --vars-template target/evidence46-l2/template.fd --expect PASS \
  --evidence-dir "$D" --net-mode present --net-user --net-protocols present --reset-bootctrl \
  --ready-marker root-heartbeat --qmp-deadline-secs 720 >"$D/run.log" 2>&1 &
QPID=$!
wait $QPID; RC=$?; case "$RC" in 124|2) RC=0;; esac  # 到点强杀=窗口完成（证据=判定行）
kill "$B" "$P_PEER" "$P_SCADA" 2>/dev/null

SERIAL="$D/qemu-serial.log"
{
  echo "=== WINDOW1 RC=$RC ==="
  echo "AUDIT-CHAIN-DURABLE lines: $(grep -c 'AUDIT-CHAIN-DURABLE' "$SERIAL" 2>/dev/null || echo 0)"
  echo "LEARN-APPLY lines:         $(grep -c 'LEARN-APPLY' "$SERIAL" 2>/dev/null || echo 0)"
  echo "LEARN-REJECT-CAP lines:    $(grep -c 'LEARN-REJECT-CAP' "$SERIAL" 2>/dev/null || echo 0)"
  echo "REMOTE-BRAIN used=true:    $(grep -c 'used=true' "$SERIAL" 2>/dev/null || echo 0)"
  echo "PEER-MSG verdict=Bad:      $(grep -c 'verdict=Bad' "$SERIAL" 2>/dev/null || echo 0)"
  echo "AUDIT-VERIFY-FAIL (must 0):$(grep -c 'AUDIT-VERIFY-FAIL' "$SERIAL" 2>/dev/null || echo 0)"
} | tee -a "$D/timeline.txt"
exit "$RC"
