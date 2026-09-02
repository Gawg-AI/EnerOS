#!/bin/bash
# 蓝图 63 卷 63.1（v0.316.0）出口 QEMU 实弹回归（2026-09-03）：
#   出口门 4（硬门）：本卷触及 root-task 显示收窄（DispatchState 码表单源
#   decode/display_name 替代内联 4 态）+ svc-control-dispatch 观测面 getter
#   （AUDIT-CHAIN 行计数来源）+ engine 结算样板收拢（ExecutionCtx）——
#   `image raw` 链重建 + qemu-prod typed 链正例一轮：
#   - PRODUCT-READY: true；
#   - CONTROL-CHAIN 终态词形逐字节不变（round=… dispatch=Committed 由单源
#     display_name 供给；dispatch=- 保持——推进态/未知码如实 "-"，零 "other"）；
#   - EXEC-JOURNAL applied-confirmed / PLAN-COMPILED 词形在（63.1.3 收拢后
#     target 实弹）；
#   - AUDIT-CHAIN appended/dropped 计数行（63.1.2 getter 读数，词形不变）。
# 证据归档 evidence/62/vol62-1 基线对照 + evidence/63/vol63-1/。脚本建模
# tools/run62-vol1-gate.sh。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/63/vol63-1"
mkdir -p "$EVROOT/build"

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_none="--sysroot=$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/aarch64-none-elf"
export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"
export ENEROS_RELEASE_PUBLIC_KEY_HEX="8e5150375a22ae2ad55ea85a3e7a8faee8196315a413163fd4778c82c59af9a7"
export ENEROS_RELEASE_KEY_ID_HEX="4e28c578ebd8c3c68b83ff1d0b1a16df"
export ENEROS_PLATFORM_ID_HEX="6afc39cf058a1ca52a5e36d8eefa1463"
K="$ROOT/product/security/keys-local"

kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（qemu-prod）==="
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --root-key-hex "$(printf '5a%.0s' {1..64})" >"$EVROOT/build/compile.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "CONFIG-COMPILE FAIL rc=$RC"; tail -5 "$EVROOT/build/compile.log"; exit 2; }
tail -2 "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接 + image raw qemu-prod（root-task 进镜像）==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/sel4.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "SEL4-RELINK FAIL rc=$RC"; tail -20 "$EVROOT/build/sel4.log"; exit 3; }
cargo xtask image raw qemu-prod \
  --signing-key "$K/dev-signing.key" --uefi-key "$K/uefi-test.key" --uefi-cert "$K/uefi-test.pem" \
  >"$EVROOT/build/image.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "IMAGE FAIL rc=$RC"; tail -20 "$EVROOT/build/image.log"; exit 3; }
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

echo "=== 步骤 3：qemu-prod typed 链正例一轮 ==="
DIR="$ROOT/target/evidence63-vol1/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
python3 "$ROOT/evidence/61/modbus_slave61.py" \
  --transcript "$ROOT/evidence/62/vol62-1/normal.modbus.transcript.jsonl" &
PPID_PEER=$!
sleep 1
if ! kill -0 $PPID_PEER 2>/dev/null; then echo "PEER-DIED"; exit 5; fi
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 600 >/dev/null 2>&1
RUNRC=$?
kill $PPID_PEER 2>/dev/null || true
cp -f "$DIR/qemu-serial.log" "$EVROOT/normal.qemu-serial.log" 2>/dev/null || true
echo "run rc=$RUNRC（124/2=超时杀=完整窗口）"

S="$DIR/qemu-serial.log"
[ -f "$S" ] || { echo "serial log missing"; exit 6; }

echo "=== 出口门判据行扫描 ==="
FAIL=0
# 词形逐字节不变（对照 evidence/62/vol62-1）：终态由单源 display_name 供给。
for pat in "PRODUCT-READY: true" \
           "CONTROL-CHAIN: round=" \
           "dispatch=Committed" \
           "PLAN-COMPILED" \
           "EXEC-JOURNAL applied-confirmed"; do
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done
# root 收窄负判据：原内联 match 的 "other" 词形不得出现（终态才显示）。
n=$(grep -c "dispatch=other" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] dispatch=other = 0（root 显示收窄）"; else echo "[✗] dispatch=other 出现 $n 次（收窄失败）"; FAIL=1; fi
# AUDIT-CHAIN 观测面 getter 行（63.1.2 读数词形不变；appended>0 证据）。
n=$(grep -c "AUDIT-CHAIN appended=" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -ge 1 ]; then echo "[✓] AUDIT-CHAIN appended= 命中=$n"; else echo "[✗] AUDIT-CHAIN appended= 缺失（观测面未出证据行）"; FAIL=1; fi

[ $FAIL -eq 0 ] && echo "VOL63.1-GATE: PASS" || { echo "VOL63.1-GATE: FAIL"; exit 7; }
echo "DONE"
