#!/bin/bash
# SPEC-GAP-CLOSURE w6：WP-1 C4/C5/C6 端到端（复用 47 卷对端三件套 × qemu-test）
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
EVIDDIR="target/evidence-wA/gate-run"; mkdir -p "$EVIDDIR"; rm -f "$EVIDDIR/qemu-serial.log"
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200
python3 evidence/47/mqtt_broker47.py --transcript target/evidence-wA/gate-run/mqtt.transcript.jsonl & P_MQTT=$!
python3 evidence/47/modbus_slave47.py --transcript target/evidence-wA/gate-run/modbus.transcript.jsonl & P_MODBUS=$!
python3 evidence/47/scada_write.py --delay 60 --retries 8 --interval 15 & P_SCADA=$!
trap 'kill $P_MQTT $P_MODBUS $P_SCADA 2>/dev/null || true' EXIT
sleep 2
cargo +nightly-2026-04-04 xtask run qemu-test \
  --image target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  --vars-template target/evidence46-l2/template.fd \
  --expect PASS --evidence-dir "$EVIDDIR" \
  --net-mode present --net-user --reset-bootctrl \
  --ready-marker root-heartbeat --qmp-deadline-secs 750
RC=$?; [ "$RC" = 124 ] && RC=0
echo "=== RC=$RC ==="
for k in TSDB-COMMIT TEL-FRAME AI-FRAG CONNACK SUBSCRIBE INTENT-FWD MASTER-WRITE Committed PRODUCT-READY; do printf '%s: ' "$k"; grep -c "$k" "$EVIDDIR/qemu-serial.log" 2>/dev/null || echo 0; done
exit $RC
