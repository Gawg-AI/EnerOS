#!/bin/bash
# 蓝图 62 卷 62.1（v0.315.0）出口 QEMU 实弹回归（2026-09-01）：
#   出口门 3（硬门）：本卷触及决策咽喉 engine.rs（结算决策表接线）+
#   plan wire SHA 单源 + svc-control-dispatch 装配一次拉取——
#   `image raw` 链重建 + qemu-prod typed 链正例一轮：
#   - PRODUCT-READY: true；
#   - PLAN-COMPILED → EXEC-JOURNAL applied-confirmed（Applied 触发经
#     settle_command 表驱动收口后的 target 实弹）；
#   - 装配词形全在（CATALOG-BINDING / REGISTRY-ACTIVE / CONTRACT-VIEW-ACTIVE
#     / CATALOG-ACTIVE——WP-62.1.4 catalog 一次拉取新签名 target 面）；
#   - fail-closed 兜底词形不得出现（REGISTRY-MISSING / CONTRACT-VIEW-MISSING
#     / CATALOG-MISSING —— 目录一次拉取成功，装配全激活）。
# 证据归档 evidence/62/vol62-1/。脚本建模 tools/run61-phase3-e4.sh。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/62/vol62-1"
mkdir -p "$EVROOT/build"

export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
K="$ROOT/product/security/keys-local"

kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（gen=1 catalog，配置与 61 卷出口一致）==="
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --root-key-hex "$(printf '5a%.0s' {1..64})" >"$EVROOT/build/compile.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "CONFIG-COMPILE FAIL rc=$RC"; tail -5 "$EVROOT/build/compile.log"; exit 2; }
tail -2 "$EVROOT/build/compile.log"

echo "=== 步骤 2：image raw qemu-prod（v0.315.0 树重建）==="
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
DIR="$ROOT/target/evidence62-vol1/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
python3 "$ROOT/evidence/61/modbus_slave61.py" \
  --transcript "$EVROOT/normal.modbus.transcript.jsonl" &
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
for pat in "PRODUCT-READY: true" \
           "CATALOG-ACTIVE gen=1" \
           "CATALOG-BINDING point=10 device=1" \
           "REGISTRY-ACTIVE binding=(1,10)" \
           "CONTRACT-VIEW-ACTIVE contracts=" \
           "PLAN-COMPILED" \
           "EXEC-JOURNAL applied-confirmed"; do
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done
# S7 target 面：目录一次拉取成功 → fail-closed 兜底词形零出现。
for bad in "REGISTRY-MISSING" "CONTRACT-VIEW-MISSING" "CATALOG-MISSING" "CHAIN-KEY-MISSING"; do
  n=$(grep -c "$bad" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] $bad = 0（装配全激活）"; else echo "[✗] $bad 出现 $n 次"; FAIL=1; fi
done
# 控制链终态词形（typed 链正例 → Committed 面）。
n=$(grep -c 'SUBMIT state=Committed' "$S" 2>/dev/null); n=${n:-0}
echo "    SUBMIT state=Committed 命中=$n"
if grep -q 'idempotent\|delivery-unreachable' "$S"; then
  echo "[!] 提示：出现幂等/unreachable 词形（非本门判据，人工过目）"
fi

[ $FAIL -eq 0 ] && echo "VOL62.1-GATE: PASS" || { echo "VOL62.1-GATE: FAIL"; exit 7; }
echo "DONE"
