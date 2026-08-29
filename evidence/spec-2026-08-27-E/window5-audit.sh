#!/bin/bash
# SPEC-E 续作 Window 5（qemu-prod 全链 · E.2-C2 + E.1-C2 + E.3-C3③）：
#   磁盘 TSDB 停用（E-DEF-01）后采集线走内存环；INGEST 经 svc-modbus-tcp
#   真实路径（振荡从站馈入）。三阶段：
#   Run1（fresh）：INGEST 落地 + SCADA 周期意图 → Committed → AUDIT-CHAIN
#                 appended；peer_brain e1-cycle 围栏/drift（mqtt 有 NIC）；
#                 LEARN-APPLY（SOC 步长驱动）。
#   Run2（--reuse 同数据盘）：AUDIT-CHAIN-DURABLE recovered（E.2-C2 续链）
#                 + LEARN-STATE restored=Run1 末值 + peer/SCADA 复跑。
#   Run3（--reuse + 审计区段中段 1 字节篡改）：AUDIT-VERIFY-FAIL（E.2-C3）
#                 + 其余功能不受累（SCADA 继续到 dispatch）。
# 用法：window5-audit.sh <out_dir> [dur_s]
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.."; pwd)"; cd "$ROOT"
D=$1; DUR=${2:-540}
export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"; export SOURCE_DATE_EPOCH=1755523200
export ENEROS_SIGN_KEY=product/security/keys-local/dev-signing.key
export ENEROS_KMS_DIR=configs/kms-dev

W=evidence/spec-2026-08-27-E/window_prod.sh
echo "=== Run1 fresh ($DUR s) ==="
bash "$W" "$D" "$DUR"
cp "$D/qemu-serial.log" "$D/qemu-serial.run1.log"

echo "=== Run2 --reuse ($DUR s) ==="
bash "$W" "$D" "$DUR" --reuse
cp "$D/qemu-serial.log" "$D/qemu-serial.run2.log"

echo "=== Run3 tamper + --reuse ($DUR s) ==="
python3 - "$D/eneros-data.raw" <<'PY'
import sys
path = sys.argv[1]
AUDIT_BASE_BLOCK = 24576
off = AUDIT_BASE_BLOCK * 512 + 1 * 1024 + 40   # slot 1 记录体（中段，非尾槽）
with open(path, "r+b") as f:
    f.seek(off)
    b = f.read(1)
    assert b and b[0] != 0, f"offset {off} 为零字节（翻转无感）——审计链可能未写入，负例无效"
    f.seek(off)
    f.write(bytes([b[0] ^ 0xFF]))
print(f"FLIPPED offset={off} byte=0x{b[0]:02x}->0x{b[0]^0xFF:02x}")
PY
bash "$W" "$D" "$DUR" --reuse
cp "$D/qemu-serial.log" "$D/qemu-serial.run3.log"

S1="$D/qemu-serial.run1.log"; S2="$D/qemu-serial.run2.log"; S3="$D/qemu-serial.run3.log"
{
  echo "=== WINDOW5 ($D) ==="
  echo "--- Run1 fresh ---"
  echo "TSDB-COMMIT count:        $(grep -c 'TSDB-COMMIT' "$S1" || true)"
  echo "INGEST round ok:          $(grep -c 'INGEST round=' "$S1" || true)"
  echo "ingest Transport(must 0): $(grep -c 'ingest transport\|INGEST eipc=Transport' "$S1" || true)"
  echo "AUDIT-CHAIN-DURABLE:      $(grep -m1 'AUDIT-CHAIN-DURABLE' "$S1" || echo NONE)"
  echo "AUDIT-CHAIN appended max: $(grep -o 'AUDIT-CHAIN appended=[0-9]*' "$S1" | tail -1 || echo none)"
  echo "Committed count:          $(grep -c 'Committed' "$S1" || true)"
  echo "LEARN-APPLY count:        $(grep -c 'LEARN-APPLY' "$S1" || true)"
  echo "LEARN-APPLY last:         $(grep 'LEARN-APPLY' "$S1" | tail -1 || echo none)"
  echo "REMOTE-BRAIN used=true:   $(grep -c 'used=true' "$S1" || true)"
  echo "PEER verdict=Bad:         $(grep -c 'verdict=Bad' "$S1" || true)"
  echo "PANIC(must 0):            $(grep -c 'PANIC' "$S1" || true)"
  echo "--- Run2 reuse (E.2-C2) ---"
  echo "AUDIT-CHAIN-DURABLE:      $(grep -m1 'AUDIT-CHAIN-DURABLE' "$S2" || echo NONE)"
  echo "AUDIT-VERIFY-FAIL(must0): $(grep -c 'AUDIT-VERIFY-FAIL' "$S2" || true)"
  echo "appended continues:       $(grep -o 'AUDIT-CHAIN appended=[0-9]*' "$S2" | tail -1 || echo none)"
  echo "LEARN-STATE:              $(grep -m1 'LEARN-STATE' "$S2" || echo NONE)"
  echo "--- Run3 tamper (E.2-C3) ---"
  echo "AUDIT-VERIFY-FAIL:        $(grep -m1 'AUDIT-VERIFY-FAIL' "$S3" || echo MISSING)"
  echo "Committed still flows:    $(grep -c 'Committed' "$S3" || true)"
} | tee "$D/timeline-window5.txt"
exit 0
