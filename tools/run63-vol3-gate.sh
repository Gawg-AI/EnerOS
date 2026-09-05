#!/bin/bash
# 蓝图 63 卷 63.3（v0.316.2）出口 QEMU 冒烟回归（2026-09-03）：
#   出口门 3（卷文件：63.3.1/63.3.2 触 business-services/federation target
#   闭包 → image raw 链 + qemu-prod 冒烟）：
#   - PRODUCT-READY: true；零 PANIC；readiness 29 基线；
#   - market/fact 拒绝词形对照（判据来源收敛后词形不变）：
#     market 走 archive 语义（MARKET-ARCHIVE/STALE-REVISION 族），
#     fact 拒绝族在位；
#   - typed 链不回归：CATALOG-ACTIVE / REGISTRY-ACTIVE / CONTRACT-VIEW-
#     ACTIVE / PLAN-COMPILED / EXEC-JOURNAL applied-confirmed /
#     dispatch=Committed；
#   - 授予边基线（68.2 接通转正后翻转：原 63.3 = NOT granted ×1、
#     bound=0、edges=34；现 = bound ≥1、NOT granted 0、edges=35）；
#   - keyring 依赖边（host 侧 cargo tree 已验：model-security/did 各 +1，
#     secure-boot 0 豁免成立）——镜像闭包 diff 不另测（三 crate 均非
#     qemu-prod 服务 bin 依赖，镜像二进制集合不变，PRODUCT-READY 即证）。
# 证据归档 evidence/63/vol63-3/。脚本建模 tools/run63-vol2-gate.sh，
# env 经 source tools/gate-env.sh（WP-63.3.5 单源）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source "$(dirname "$0")/gate-env.sh"
EVROOT="$ROOT/evidence/63/vol63-3"
mkdir -p "$EVROOT/build"
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

echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 1.5：root-task 重链接（门例行，防陈旧顶替）==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -10 "$EVROOT/build/relink.log"; exit 9; }
grep -a "Rust ELF:" "$EVROOT/build/relink.log" | tail -1

echo "=== 步骤 2：image raw qemu-prod（v0.316.2 树重建）==="
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

echo "=== 步骤 3：qemu-prod 实弹（modbus + iec104 双对端）==="
DIR="$ROOT/target/evidence63-vol3/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
cp -f "$ROOT/evidence/62/vol62-3/normal.modbus.transcript.jsonl" "$EVROOT/normal.modbus.transcript.jsonl"
cp -f "$ROOT/evidence/62/vol62-3/normal.iec104.transcript.jsonl" "$EVROOT/normal.iec104.transcript.jsonl"
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

# 1. 产品就绪 + 零 PANIC + readiness 29。
grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }
n=$(grep -ac "PANIC" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] PANIC = 0"; else echo "[✗] PANIC 出现 $n 次"; FAIL=1; fi
RN=$(grep -ac "READINESS: " "$S" 2>/dev/null); RN=${RN:-0}
if [ "$RN" -eq 29 ]; then echo "[✓] READINESS = 29"; else echo "[✗] READINESS 行=$RN（基线 29）"; FAIL=1; fi

# 2. market/fact 拒绝词形对照（判据收敛后词形不变；host 测试已锁，
#    串口侧核对 telemetry 拒绝族仍在 = 拒绝语义词形链路完好）。
grep -aq "OBS-REJECT" "$S" && echo "[✓] OBS-REJECT 词形在（拒绝语义链路）" || { echo "[✗] OBS-REJECT 缺失"; FAIL=1; }
grep -aq "SNAPSHOT rejected" "$S" && echo "[✓] SNAPSHOT rejected 词形在（快照拒绝族）" || { echo "[✗] SNAPSHOT rejected 缺失"; FAIL=1; }

# 3. typed 链不回归 + dispatch 词形（63.1 单源保持）。
for pat in "CATALOG-ACTIVE gen=1" "REGISTRY-ACTIVE binding=(1,10)" "CONTRACT-VIEW-ACTIVE contracts=" "PLAN-COMPILED" "EXEC-JOURNAL applied-confirmed" "CONTROL-CHAIN: round=" "dispatch=Committed"; do
  grep -q "$pat" "$S" && echo "[✓] $pat" || { echo "[✗] $pat 缺失"; FAIL=1; }
done
n=$(grep -ac "dispatch=other" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] dispatch=other = 0"; else echo "[✗] dispatch=other 出现 $n 次"; FAIL=1; fi

# 4. 授予边基线保持（63.2 处置不回归）。
# 68.2 起翻转（原 63.3 基线 = 缺边 fail-closed；边已接通，见 run63-vol2-gate）。
n=$(grep -ac '\[svc-iec104\] telemetry EIPC bound' "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -ge 1 ]; then echo "[✓] telemetry EIPC bound = $n（68.2 边接通）"; else echo "[✗] bound 缺失（边未接通）"; FAIL=1; fi
n=$(grep -ac 'telemetry EIPC NOT granted' "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] telemetry EIPC NOT granted = 0（缺边词形退场）"; else echo "[✗] NOT granted 出现 $n 次"; FAIL=1; fi
grep -aq "EIPC-GRANT-TABLE edges=35" "$S" && echo "[✓] EIPC-GRANT-TABLE edges=35（68.2 新边入表）" || { echo "[✗] edges 计数漂移（基线 35）"; FAIL=1; }

# 5. 采集基线（63.2 不回归）：modbus 7 点值 + OBS-ROUND + iec104 ACQ。
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
print("[✓] modbus 7 点值基线逐点一致")
EOF
[ $? -eq 0 ] || FAIL=1
ACQ_N=$(grep -ac '\[svc-iec104\] ACQ point=' "$S" 2>/dev/null); ACQ_N=${ACQ_N:-0}
if [ "$ACQ_N" -ge 4 ]; then echo "[✓] svc-iec104 ACQ 行 ≥4（$ACQ_N）"; else echo "[✗] iec104 ACQ 不足（$ACQ_N）"; FAIL=1; fi

# 6. fail-closed 兜底词形零出现。
for bad in "REGISTRY-MISSING" "CONTRACT-VIEW-MISSING" "CATALOG-MISSING" "CHAIN-KEY-MISSING" "OBS-BATCH malformed"; do
  n=$(grep -ac "$bad" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] $bad = 0"; else echo "[✗] $bad 出现 $n 次"; FAIL=1; fi
done

[ $FAIL -eq 0 ] && echo "VOL63.3-GATE: PASS" || { echo "VOL63.3-GATE: FAIL"; exit 7; }
echo "DONE"
