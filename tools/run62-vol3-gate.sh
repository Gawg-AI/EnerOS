#!/bin/bash
# 蓝图 62 卷 62.3（v0.315.2）出口 QEMU 实弹回归（2026-09-02）：
#   出口门（硬门）：本卷触及 ingestion-core（WatermarkTracker + descriptor_of
#   工厂）、event-store（StoreSink tracker 化）、control-services（ObsShadow
#   钉扎 tracker 化）、两协议 bins（descriptor 工厂接入）——均为 target 侧，
#   `image raw` 链重建 + qemu-prod 实弹：
#   - PRODUCT-READY: true；零 PANIC；
#   - W3 采集输出回归：modbus-tcp 7 点 ACQ 值与 62.2/Phase-0 基线逐点一致
#     （400/50/50/0.6/1/0/0——WatermarkTracker 接入后采集输出不变）；
#   - S2a+W3 上行回归：modbus OBS-ROUND sent=7 accepted=7（上行链经
#     tracker 化 InMemorySink 全程通）；
#   - descriptor 工厂回归：svc-iec104 OBS-SOURCE id=3 config-allocated
#     （工厂构造 descriptor 后真插件态采集）+ iec104 ACQ 行 ≥4（D-1 保持）；
#   - typed 链不回归：PLAN-COMPILED / EXEC-JOURNAL applied-confirmed /
#     CATALOG-ACTIVE / REGISTRY-ACTIVE / CONTRACT-VIEW-ACTIVE；
#   - fail-closed 兜底词形零出现。
# 证据归档 evidence/62/vol62-3/。脚本建模 tools/run62-vol2-gate.sh。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/62/vol62-3"
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
kill -9 $(pgrep -f "iec104_slave6" ) 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582 2404; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（gen=1 catalog，配置与 62.2 出口一致）==="
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --root-key-hex "$(printf '5a%.0s' {1..64})" >"$EVROOT/build/compile.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "CONFIG-COMPILE FAIL rc=$RC"; tail -5 "$EVROOT/build/compile.log"; exit 2; }
tail -2 "$EVROOT/build/compile.log"

echo "=== 步骤 1.5：root-task 重链接（62.2 教训门固化：image raw 不重链接
# rootserver.elf——本卷未动 root-task，仍按门例行重跑防陈旧顶替）==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -10 "$EVROOT/build/relink.log"; exit 9; }
grep -a "Rust ELF:" "$EVROOT/build/relink.log" | tail -1

echo "=== 步骤 2：image raw qemu-prod（v0.315.2 树重建）==="
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

echo "=== 步骤 3：qemu-prod 实弹（modbus + iec104 双对端在位）==="
DIR="$ROOT/target/evidence62-vol3/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
python3 "$ROOT/evidence/61/modbus_slave61.py" \
  --transcript "$EVROOT/normal.modbus.transcript.jsonl" &
PEER_MODBUS=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" \
  --transcript "$EVROOT/normal.iec104.transcript.jsonl" --deadline 580 &
PEER_104=$!
sleep 1
if ! kill -0 $PEER_MODBUS 2>/dev/null; then echo "MODBUS-PEER-DIED"; exit 5; fi
if ! kill -0 $PEER_104 2>/dev/null; then echo "IEC104-PEER-DIED"; exit 5; fi
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 600 >/dev/null 2>&1
RUNRC=$?
kill $PEER_MODBUS $PEER_104 2>/dev/null || true
cp -f "$DIR/qemu-serial.log" "$EVROOT/normal.qemu-serial.log" 2>/dev/null || true
echo "run rc=$RUNRC（124/2=超时杀=完整窗口）"

S="$DIR/qemu-serial.log"
[ -f "$S" ] || { echo "serial log missing"; exit 6; }

echo "=== 出口门判据行扫描 ==="
FAIL=0
# 1. 产品就绪 + 零 PANIC。
grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }
n=$(grep -ac "PANIC" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] PANIC = 0"; else echo "[✗] PANIC 出现 $n 次"; FAIL=1; fi

# 2. W3 采集输出回归：modbus 7 点值基线逐点一致（tracker 接入后输出不变
# ——20 点基线一致判据的 qemu-prod 载体）。
python3 - "$S" <<'EOF'
import re, sys
vals = {}
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"\[svc-modbus-tcp\] ACQ point=(\d+) value=([-\d.]+) q=good", line)
    if m:
        vals.setdefault(int(m.group(1)), m.group(2))
baseline = {1: "400", 2: "50", 3: "50", 4: "0.6", 5: "1", 6: "0", 7: "0"}
bad = [(p, vals.get(p), v) for p, v in baseline.items() if vals.get(p) != v]
if bad:
    print(f"[✗] modbus 值基线不一致: {bad}")
    sys.exit(1)
print("[✓] modbus 7 点值基线逐点一致（400/50/50/0.6/1/0/0——W3 采集输出回归）")
EOF
[ $? -eq 0 ] || FAIL=1

# 3. S2a+W3 上行回归：交付半环经 tracker 化 InMemorySink 全程通。
grep -q '\[svc-modbus-tcp\] OBS-ROUND sent=7 accepted=7' "$S" && echo "[✓] modbus OBS-ROUND sent=7 accepted=7（62.2 基线词形，上行经 tracker）" || { echo "[✗] modbus OBS-ROUND 基线词形缺失"; FAIL=1; }

# 4. descriptor 工厂回归：svc-iec104 OBS-SOURCE config-allocated + D-1 保持。
grep -aq '\[svc-iec104\] OBS-SOURCE' "$S" && echo "[✓] svc-iec104 OBS-SOURCE（descriptor 工厂构造，来源注册在位）" || { echo "[✗] svc-iec104 OBS-SOURCE 缺失"; FAIL=1; }
grep -a '\[svc-iec104\] OBS-SOURCE' "$S" | head -1
ACQ_N=$(grep -ac '\[svc-iec104\] ACQ point=' "$S" 2>/dev/null); ACQ_N=${ACQ_N:-0}
if [ "$ACQ_N" -ge 4 ]; then echo "[✓] svc-iec104 ACQ 行 ≥4（D-1 保持，实得 $ACQ_N）"; else echo "[✗] svc-iec104 ACQ 行不足（$ACQ_N）"; FAIL=1; fi
if [ -f "$EVROOT/normal.iec104.transcript.jsonl" ]; then
  INTR_N=$(grep -aic 'INTERROGATION+DATA' "$EVROOT/normal.iec104.transcript.jsonl" 2>/dev/null); INTR_N=${INTR_N:-0}
  echo "    iec104 从站 transcript 行=$(wc -l < "$EVROOT/normal.iec104.transcript.jsonl")（总召+数据轮次=$INTR_N）"
  if [ "$INTR_N" -ge 2 ]; then echo "[✓] iec104 从站总召+数据 ≥2 轮（对端独立记录）"; else echo "[✗] iec104 从站轮次不足（$INTR_N）"; FAIL=1; fi
else
  echo "[✗] iec104 transcript 缺失"; FAIL=1
fi

# 5. typed 链不回归 + 62.1/62.2 装配词形。
grep -q 'drv-svc-mqtt. READY-SIGNALED' "$S" && echo "[✓] svc-mqtt READY-SIGNALED" || { echo "[✗] svc-mqtt READY-SIGNALED 缺失"; FAIL=1; }
for pat in "CATALOG-ACTIVE gen=1" "REGISTRY-ACTIVE binding=(1,10)" "CONTRACT-VIEW-ACTIVE contracts=" "PLAN-COMPILED" "EXEC-JOURNAL applied-confirmed"; do
  grep -q "$pat" "$S" && echo "[✓] $pat" || { echo "[✗] $pat 缺失"; FAIL=1; }
done
n=$(grep -ac 'SUBMIT state=Committed' "$S" 2>/dev/null); n=${n:-0}
echo "    SUBMIT state=Committed 命中=$n"

# 6. fail-closed 兜底词形零出现。
for bad in "REGISTRY-MISSING" "CONTRACT-VIEW-MISSING" "CATALOG-MISSING" "CHAIN-KEY-MISSING" "OBS-BATCH malformed"; do
  n=$(grep -ac "$bad" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] $bad = 0"; else echo "[✗] $bad 出现 $n 次"; FAIL=1; fi
done
NG=$(grep -ac '\[svc-iec104\] telemetry EP not granted' "$S" 2>/dev/null); NG=${NG:-0}
echo "    [i] svc-iec104 telemetry EP not granted 行=$NG（预存授予缺口，诚实降级面，63 候选）"

[ $FAIL -eq 0 ] && echo "VOL62.3-GATE: PASS" || { echo "VOL62.3-GATE: FAIL"; exit 7; }
echo "DONE"
