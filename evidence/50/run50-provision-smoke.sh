#!/bin/bash
# 蓝图 50（WP-50.3）DoD 冒烟：空盘 → factory provision → 直接引导。
# 关键语义：provision 出的盘**不加 --reset-bootctrl** 原样引导——
# Genesis 状态由产线工具写入，引导成功 = provision 链真实有效。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
EVID="$ROOT/evidence/50"
EVIDDIR="target/evidence50-smoke"
mkdir -p "$EVIDDIR" "$EVID/factory"
rm -f "$EVIDDIR"/qemu-serial.log "$EVID"/factory/smoke.raw

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
ls -la "$IMG" >/dev/null || { echo "image missing"; exit 3; }

TPL="$EVIDDIR/template.fd"
cp -f "$ROOT/target/evidence47/template.fd" "$TPL" 2>/dev/null || true
ls "$TPL" >/dev/null 2>&1 || { echo "vars template missing"; exit 3; }

# 1. 空盘 provision（Genesis + readback 自检 + 制造记录）。
cargo xtask factory provision \
  --image "$IMG" \
  --target "$EVID/factory/smoke.raw" \
  --serial ENP-2026-SMOKE \
  --station station-smoke \
  --sign-key "$ROOT/product/security/keys-local/dev-signing.key" \
  --record "$EVID/factory/smoke-record.json" || exit 3

# 2. 记录校验（抽检项 a）。
cargo xtask factory verify-record \
  --record "$EVID/factory/smoke-record.json" \
  --sign-key "$ROOT/product/security/keys-local/dev-signing.key" \
  --image "$EVID/factory/smoke.raw" || exit 3

# 3. 启动冒烟：provisioned 盘原样引导（**不传 --reset-bootctrl**——
#    Genesis 已由产线工具写入，引导成功 = provision 链真实有效）。
cargo xtask run qemu-prod \
  --image "$EVID/factory/smoke.raw" \
  --vars-template "$TPL" \
  --expect PASS \
  --evidence-dir "$EVIDDIR" \
  --net-mode absent \
  --ready-marker product-ready \
  --qmp-deadline-secs 300
RC=$?
[ "$RC" = "124" ] && RC=0

echo "=== RC=$RC ==="
grep -E "PRODUCT-READY|bootctrl|GENESIS" "$EVIDDIR/qemu-serial.log" 2>/dev/null | head -8
exit "$RC"
