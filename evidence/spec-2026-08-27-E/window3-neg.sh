#!/bin/bash
# SPEC-E Window 3：E.2 C3 负例——审计区段**中段**单字节篡改 ⇒ 重启
# AUDIT-VERIFY-FAIL（fail-stop 定位）+ 系统其余功能不受累（SCADA 写继续
# Committed = 隔离降级实证；dropped 计数上行 = 停止追加实证）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.."; pwd)"; cd "$ROOT"
D=target/evidence-e/window2           # 复用 W2 后的数据盘（链上已有多条）
D3=target/evidence-e/window3-neg; mkdir -p "$D3"
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200

DISK="$D/eneros-data.raw"
test -s "$DISK" || { echo "data disk missing: $DISK"; exit 2; }

# 篡改：审计区段（AUDIT_CHAIN_BASE_BLOCK=24576）槽 1 记录体 1 字节
# （中段翻转——W2 判据行偏差-3 的定位面；尾槽翻转属撕裂容忍窗，不用）。
python3 - "$DISK" <<'PY'
import sys
path = sys.argv[1]
AUDIT_BASE_BLOCK = 24576
off = AUDIT_BASE_BLOCK * 512 + 1 * 1024 + 40   # slot 1（第 2 条）记录体
with open(path, "r+b") as f:
    f.seek(off)
    b = f.read(1)
    assert b and b[0] != 0, f"offset {off} 为零字节（翻转无感），改选中段已写字节"
    f.seek(off)
    f.write(bytes([b[0] ^ 0xFF]))
print(f"FLIPPED offset={off} byte=0x{b[0]:02x}->0x{b[0]^0xFF:02x}")
PY
echo "FLIP-DONE" | tee "$D3/timeline.txt"

python3 evidence/47/mqtt_broker47.py --transcript "$D3/broker.jsonl" &
B=$!
sleep 2
python3 evidence/47/scada_write.py --delay 40 --retries 4 --interval 20 \
  --payload "SET 1 10 70.0" > "$D3/scada.out" 2>&1 & P_SCADA=$!

cargo +nightly-2026-04-04 xtask run qemu-test \
  --image target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  --vars-template target/evidence46-l2/template.fd --expect PASS \
  --evidence-dir "$D" --net-mode present --net-user --net-protocols present --reset-bootctrl \
  --ready-marker root-heartbeat --qmp-deadline-secs 480 >"$D3/run.log" 2>&1 &
QPID=$!
wait $QPID; RC=$?; case "$RC" in 124|2) RC=0;; esac  # 到点强杀=窗口完成（证据=判定行）
kill "$B" "$P_SCADA" 2>/dev/null

SERIAL="$D/qemu-serial.log"
{
  echo "=== WINDOW3 RC=$RC ==="
  echo "AUDIT-VERIFY-FAIL:        $(grep -m1 'AUDIT-VERIFY-FAIL' "$SERIAL" 2>/dev/null || echo MISSING)"
  echo "post-fail appended (must 0): $(grep -o 'AUDIT-CHAIN appended=[0-9]*' "$SERIAL" | head -1)"
  echo "SCADA still Committed:    $(grep -c 'Committed' "$SERIAL" 2>/dev/null || echo 0)"
} | tee -a "$D3/timeline.txt"
exit "$RC"
