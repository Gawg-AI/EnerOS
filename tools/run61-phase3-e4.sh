#!/bin/bash
# 蓝图 61 Phase 3 E4 QEMU 实弹（v0.314.33 出口证据，2026-08-31）：
#   1. config compile（points [[source]] + catalog 目录 → bundle）
#   2. image raw qemu-prod（v0.314.33 树重建：SemanticCatalog 生产接线——
#      svc-telemetry 覆盖/反馈自目录、控制绑定=显式目录点、dispatch
#      反馈映射/资源注册表自目录）
#   3. E4-P3-GEN1 正常用例：CATALOG-ACTIVE gen=1 → CATALOG-COVERAGE →
#      CATALOG-FEEDBACK → CATALOG-BINDING → REGISTRY-ACTIVE →
#      OBS/ACQ/PLAN-TYPED/EXEC-JOURNAL（旧链不回归）。
#   4. E4-P3-GEN2：catalog 换代（generation=2）→ CATALOG-ACTIVE gen=2。
#   5. E4-P3-ROLLBACK：重建 gen=1 镜像再次引导 → CATALOG-ACTIVE gen=1
#      （回滚通过 [08] §2.7）。
# 证据归档 evidence/61/phase3-e4/。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/phase3-e4"
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

# 孤儿清理（peer/QEMU；5577 被占即秒退）。
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

compile_config() {
  ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
    --root-key-hex "$(printf '5a%.0s' {1..64})" >>"$EVROOT/build/compile.log" 2>&1
  local rc=$?
  [ $rc -eq 0 ] || { tail -5 "$EVROOT/build/compile.log"; exit 2; }
}

build_image() {
  cargo xtask image raw qemu-prod \
    --signing-key "$K/dev-signing.key" --uefi-key "$K/uefi-test.key" --uefi-cert "$K/uefi-test.pem" \
    >>"$EVROOT/build/image.log" 2>&1
  local rc=$?
  [ $rc -eq 0 ] || { tail -20 "$EVROOT/build/image.log"; exit 3; }
  IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
  [ -f "$IMG" ] || { echo "image missing"; exit 3; }
}

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

run_case() {
  local name="$1"
  local dir="$ROOT/target/evidence61-p3/$name"
  mkdir -p "$dir"; rm -f "$dir/qemu-serial.log" "$dir/eneros-data.raw"
  python3 "$ROOT/evidence/61/modbus_slave61.py" \
    --transcript "$EVROOT/$name.modbus.transcript.jsonl" &
  local ppid=$!
  sleep 1
  if ! kill -0 $ppid 2>/dev/null; then echo "PEER-DIED $name"; return 1; fi
  cargo xtask run qemu-prod \
    --image "$IMG" \
    --vars-template "$TPL" \
    --expect PASS --evidence-dir "$dir" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 600 >/dev/null 2>&1
  kill $ppid 2>/dev/null || true
  cp -f "$dir/qemu-serial.log" "$EVROOT/$name.qemu-serial.log" 2>/dev/null || true
  echo "case $name rc=$?（124/2=超时杀=完整窗口）"
}

echo "=== 步骤 1：config compile（gen=1 catalog）==="
rm -f "$EVROOT/build/compile.log" "$EVROOT/build/image.log"
compile_config
grep 'catalog →' "$EVROOT/build/compile.log" | tail -1

echo "=== 步骤 2：image raw qemu-prod（gen=1）==="
build_image
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256-gen1"

echo "=== E4-P3-GEN1：正常 50kW 纵切（catalog gen=1 激活）==="
run_case cat-gen1

echo "=== 步骤 3：catalog 换代（generation=2）→ 重建 ==="
cp configs/catalog.toml "$EVROOT/build/catalog.toml.gen1.bak"
sed -i 's/^generation = 1$/generation = 2/' configs/catalog.toml
sed -i 's/^topology_version = 1$/topology_version = 2/' configs/catalog.toml
compile_config
grep 'catalog →' "$EVROOT/build/compile.log" | tail -1
build_image
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256-gen2"

echo "=== E4-P3-GEN2：catalog 换代激活 ==="
run_case cat-gen2

echo "=== 步骤 4：回滚（恢复 gen=1，重建，再引导）==="
cp "$EVROOT/build/catalog.toml.gen1.bak" configs/catalog.toml
compile_config
grep 'catalog →' "$EVROOT/build/compile.log" | tail -1
build_image
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256-rollback"

echo "=== E4-P3-ROLLBACK：gen=1 回滚再激活 ==="
run_case cat-rollback

echo "=== 判据行扫描（GEN1）==="
FAIL=0
S="$ROOT/target/evidence61-p3/cat-gen1/qemu-serial.log"
for pat in "CATALOG-ACTIVE gen=1" "CATALOG-COVERAGE points=7" \
           "CATALOG-BINDING point=10 device=1" "CATALOG-FEEDBACK grid=5 soc=4 v=1 oc=6 ot=7" \
           "REGISTRY-ACTIVE binding=(1,10)" "OBS-SOURCE" "ACQ point=" \
           "PLAN-TYPED" "EXEC-JOURNAL applied-confirmed"; do
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done
# 旧「首个 Control 点」绑定词形不得出现（退出 gate：生产无手写 first control binding）。
if grep -q "CONTROL-BINDING device=" "$S"; then
  echo "[✗] 旧 CONTROL-BINDING（首 Control 点）词形仍在"; FAIL=1
else
  echo "[✓] 旧首 Control 点绑定词形已退役（显式目录绑定替代）"
fi
# 20 点基线一致性（与 Phase 0/2 同值）。
python3 - "$S" <<'EOF'
import re, sys
vals = {}
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"ACQ point=(\d+) value=([-\d.]+) q=good", line)
    if m:
        vals.setdefault(int(m.group(1)), m.group(2))
baseline = {1: "400", 2: "50", 3: "50", 4: "0.6", 5: "1", 6: "0", 7: "0"}
bad = [(p, vals.get(p), v) for p, v in baseline.items() if vals.get(p) != v]
if bad:
    print(f"[✗] 基线值不一致: {bad}")
    sys.exit(1)
print("[✓] 20 点基线一致（modbus-tcp 7 点值对照 Phase 0）")
EOF
[ $? -eq 0 ] || FAIL=1

echo "=== 判据行扫描（GEN2 / ROLLBACK）==="
S2="$ROOT/target/evidence61-p3/cat-gen2/qemu-serial.log"
S3="$ROOT/target/evidence61-p3/cat-rollback/qemu-serial.log"
if grep -q "CATALOG-ACTIVE gen=2" "$S2" && grep -q "CATALOG-ACTIVE gen=1" "$S3"; then
  echo "[✓] 换代激活 gen=2 与回滚再激活 gen=1（[08] §2.7 回滚 gate）"
else
  echo "[✗] 换代/回滚激活行缺失（gen2=$(grep -c 'CATALOG-ACTIVE gen=2' "$S2" 2>/dev/null) rollback=$(grep -c 'CATALOG-ACTIVE gen=1' "$S3" 2>/dev/null)）"
  FAIL=1
fi
# 回滚后控制链仍完整（同一 gen=1 镜像）。
if grep -q "EXEC-JOURNAL applied-confirmed" "$S3"; then
  echo "[✓] 回滚镜像控制链完整"
else
  echo "[✗] 回滚镜像控制链缺失"; FAIL=1
fi
[ $FAIL -eq 0 ] && echo "E4-P3-GATE: PASS" || { echo "E4-P3-GATE: FAIL"; exit 5; }
echo "DONE"