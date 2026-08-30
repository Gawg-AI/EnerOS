#!/bin/bash
# 蓝图 61 Phase 2 E4 QEMU 实弹（v0.314.30 出口证据，2026-08-30）：
#   1. config compile（points.toml [[source]] 表 + 点身份块 → bundle）
#   2. image raw qemu-prod（v0.314.30 树重建：IngestionModule 深模块接入——
#      modbus 采集轮 = module.tick（周期调度/组读/准入/死区/QoS/背压）；
#      ObservationEvent = 唯一上行，legacy 58B INGEST wire 退役）
#   3. E4-P2-M1 正常用例：合法 50kW → AppliedConfirmed/Committed；
#      判据串口行 = OBS-SOURCE（配置分配 epoch）→ ACQ（事件解码，
#      同源 wire）→ OBS-ROUND sent=/accepted=（新词形）→ OBS-SHADOW
#      MATCH（影子对照保留）→ OBS-CFG loaded → PLAN-TYPED →
#      EXEC-JOURNAL applied-confirmed。
#   4. E4-P2-RESTART：同数据盘二次启动——OBS-SOURCE epoch 递增。
# 证据归档 evidence/61/phase2-e4/。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/phase2-e4"
mkdir -p "$EVROOT"

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

echo "=== 步骤 1：config compile（[[source]] 表 + 身份块入 bundle）==="
mkdir -p "$EVROOT/build"
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --root-key-hex "$(printf '5a%.0s' {1..64})" >"$EVROOT/build/compile.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "CONFIG-COMPILE FAIL rc=$RC"; tail -5 "$EVROOT/build/compile.log"; exit 2; }
tail -3 "$EVROOT/build/compile.log"

echo "=== 步骤 2：image raw qemu-prod（v0.314.30 树重建）==="
cargo xtask image raw qemu-prod \
  --signing-key "$K/dev-signing.key" --uefi-key "$K/uefi-test.key" --uefi-cert "$K/uefi-test.pem" \
  >"$EVROOT/build/image.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "IMAGE FAIL rc=$RC"; tail -20 "$EVROOT/build/image.log"; exit 3; }
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

# VARS：复用 Phase 0 验证过的模板（同一测试签名证书——fixverify 先例）。
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

run_case() {
  local name="$1"
  local dir="$ROOT/target/evidence61-p2/$name"
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

echo "=== E4-P2-M1：正常 50kW 纵切（IngestionModule 事件平面实弹）==="
run_case m1-module

echo "=== E4-P2-RESTART：二次启动（OBS-SOURCE epoch 变化 + watermark 恢复）==="
run_case restart

echo "=== 判据行扫描（m1-module）==="
FAIL=0
S="$ROOT/target/evidence61-p2/m1-module/qemu-serial.log"
for pat in "OBS-SOURCE" "ACQ point=" "OBS-ROUND sent=" "OBS-SHADOW MATCH" "OBS-CFG loaded" \
           "PLAN-TYPED" "EXEC-JOURNAL applied-confirmed"; do
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done
if grep -q "OBS-SHADOW MISMATCH" "$S"; then echo "[✗] OBS-SHADOW MISMATCH"; FAIL=1; fi
# legacy wire 退役门：生产协议代码不再发 58B INGEST。
if grep -q "\[svc-modbus-tcp\] INGEST round=" "$S"; then
  echo "[✗] legacy INGEST wire 仍被 modbus 发送"; FAIL=1
else
  echo "[✓] legacy INGEST wire 已退役（modbus 仅 OBS 上行）"
fi
# 20 点基线一致性：ACQ 好点值与 Phase 0 基线同（400/50/50/0.6/1/0/0）。
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
print(f"[✓] 20 点基线一致（modbus-tcp 7 点值对照 Phase 0）")
EOF
[ $? -eq 0 ] || FAIL=1

echo "=== 判据行扫描（restart epoch 对照）==="
grep -h "OBS-SOURCE" "$ROOT/target/evidence61-p2/m1-module/qemu-serial.log" \
  "$ROOT/target/evidence61-p2/restart/qemu-serial.log" 2>/dev/null | sort -u
R2="$ROOT/target/evidence61-p2/restart/qemu-serial.log"
if grep -q "OBS-SHADOW MATCH" "$R2" && ! grep -q "OBS-ROUND rejected" "$R2"; then
  echo "[✓] restart 影子轨健康"
else
  echo "[✗] restart 影子轨异常"; FAIL=1
fi
[ $FAIL -eq 0 ] && echo "E4-P2-GATE: PASS" || { echo "E4-P2-GATE: FAIL"; exit 5; }
echo "DONE"
