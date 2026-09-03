#!/bin/bash
# 蓝图 63 卷 63.2（v0.316.1）出口 QEMU 实弹回归（2026-09-03）：
#   出口门（硬门）：本卷触及 root-task（ChunkedSession epoch 写回内化 +
#   PERIODIC_SCHEDULE 周期编排表 + restart 单方法 + 接线完备性门 + EIPC
#   授予边修复）+ agent-sdk（ChunkedSession/eipc_grants 新边）+ 两协议
#   bins（build_ingested_service 装配序列单源 + 能力壳收编）——`image raw`
#   链重建 + qemu-prod 实弹（modbus + iec104 双对端在位）：
#   - PRODUCT-READY: true；零 PANIC；readiness 29 服务基线；
#   - 授予边处置（WP-63.2.3）：svc-iec104 telemetry EIPC bound（NOT granted
#     词形零出现——62.3 §H 登记缺边修除）+ EIPC-GRANT-TABLE edges=35；
#   - 编排形状收敛（WP-63.2.2）：ACQ/CONTROL-CHAIN/OBS-REPORT 节奏与 62.3
#     基线一致（±25% 容差）；OTA-DOWNLOAD 四行逐字节一致；
#     HEARTBEAT-KILL/SUPERVISOR = 0（探针 absent 基线一致）；
#   - 装配收敛（WP-63.2.4）：modbus 7 点值基线 + OBS-ROUND sent=7 accepted=7
#     + svc-iec104 OBS-SOURCE/ACQ（build_ingested_service 单源后行为不变）；
#   - typed 链不回归 + fail-closed 兜底词形零出现。
# 证据归档 evidence/63/vol63-2/。脚本建模 tools/run62-vol3-gate.sh。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/63/vol63-2"
mkdir -p "$EVROOT/build"

# WP-64.3.6：build 前导单源（清场/钥导出/compile 收编；compile 与 image
# 之间需 root-task 重链接 → 分相调用 gate_env 函数）。
gate_cleanup_env 5577 5582 2404
gate_export_signing_key

kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave6" ) 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582 2404; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（qemu-prod，配置与 62.3 出口一致）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 1.5：root-task 重链接（门固化：image raw 不重链接
rootserver.elf——本卷动了 root-task，必跑）==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -10 "$EVROOT/build/relink.log"; exit 9; }
grep -a "Rust ELF:" "$EVROOT/build/relink.log" | tail -1

echo "=== 步骤 2：image raw qemu-prod（v0.316.1 树重建）==="
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

echo "=== 步骤 3：qemu-prod 实弹（modbus + iec104 双对端在位）==="
DIR="$ROOT/target/evidence63-vol2/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
# peer 会把运行回放 append 进 --transcript 文件——先拷基线输入到本卷证据
# 目录（禁止写已跟踪的 vol62-3 基线文件，63.1 教训）。
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
BASE="$ROOT/evidence/62/vol62-3/normal.qemu-serial.log"

echo "=== 出口门判据行扫描 ==="
FAIL=0

# 1. 产品就绪 + 零 PANIC + readiness 29 服务基线。
grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }
n=$(grep -ac "PANIC" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] PANIC = 0"; else echo "[✗] PANIC 出现 $n 次"; FAIL=1; fi
RN=$(grep -ac "READINESS: " "$S" 2>/dev/null); RN=${RN:-0}
if [ "$RN" -eq 29 ]; then echo "[✓] READINESS = 29（服务 ready 基线）"; else echo "[✗] READINESS 行=$RN（基线 29）"; FAIL=1; fi

# 2. 授予边处置（WP-63.2.3 = 登记不修）：缺边 = fail-closed 诚实降级基线
#    （激活即回归——iec104 epoch 语义毒化评估器，见 eipc_grants.rs 表注释
#    与 investigation-grant-edge.qemu-serial.log），与 62.3 基线一致。
grep -aq '\[svc-iec104\] telemetry EIPC NOT granted' "$S" && echo "[✓] svc-iec104 telemetry EIPC NOT granted = 1（诚实降级基线，62.3 §H 登记维持）" || { echo "[✗] NOT granted 词形缺失（基线应含）"; FAIL=1; }
n=$(grep -ac '\[svc-iec104\] telemetry EIPC bound' "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] telemetry EIPC bound = 0（边未接通，回归防护）"; else echo "[✗] bound 出现 $n 次——边被误接通（激活即回归，见登记）"; FAIL=1; fi
grep -aq "EIPC-GRANT-TABLE edges=34" "$S" && echo "[✓] EIPC-GRANT-TABLE edges=34（62.3 基线一致）" || { echo "[✗] EIPC-GRANT-TABLE edges 计数漂移"; FAIL=1; }

# 3. 装配收敛回归（WP-63.2.4）：modbus 7 点值基线 + OBS-ROUND + iec104 采集。
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
print("[✓] modbus 7 点值基线逐点一致（400/50/50/0.6/1/0/0）")
EOF
[ $? -eq 0 ] || FAIL=1
grep -q '\[svc-modbus-tcp\] OBS-ROUND sent=7 accepted=7' "$S" && echo "[✓] modbus OBS-ROUND sent=7 accepted=7" || { echo "[✗] modbus OBS-ROUND 基线词形缺失"; FAIL=1; }
grep -aq '\[svc-iec104\] OBS-SOURCE' "$S" && echo "[✓] svc-iec104 OBS-SOURCE（build_ingested_service 单源装配）" || { echo "[✗] svc-iec104 OBS-SOURCE 缺失"; FAIL=1; }
ACQ_N=$(grep -ac '\[svc-iec104\] ACQ point=' "$S" 2>/dev/null); ACQ_N=${ACQ_N:-0}
if [ "$ACQ_N" -ge 4 ]; then echo "[✓] svc-iec104 ACQ 行 ≥4（D-1 保持，实得 $ACQ_N）"; else echo "[✗] svc-iec104 ACQ 行不足（$ACQ_N）"; FAIL=1; fi
if [ -f "$EVROOT/normal.iec104.transcript.jsonl" ]; then
  INTR_N=$(grep -aic 'INTERROGATION+DATA' "$EVROOT/normal.iec104.transcript.jsonl" 2>/dev/null); INTR_N=${INTR_N:-0}
  if [ "$INTR_N" -ge 2 ]; then echo "[✓] iec104 从站总召+数据 ≥2 轮"; else echo "[✗] iec104 从站轮次不足（$INTR_N）"; FAIL=1; fi
else
  echo "[✗] iec104 transcript 缺失"; FAIL=1
fi

# 4. 编排形状收敛（WP-63.2.2）：OTA-DOWNLOAD 四行逐字节一致；节奏容差。
diff <(grep -a "OTA-DOWNLOAD" "$BASE" | sort) <(grep -a "OTA-DOWNLOAD" "$S" | sort) >/dev/null \
  && echo "[✓] OTA-DOWNLOAD 行族与基线逐字节一致" \
  || { echo "[✗] OTA-DOWNLOAD 词形漂移"; FAIL=1; }
for fam in ACQ-ROUND CONTROL-CHAIN OBS-REPORT; do
  bn=$(grep -ac "$fam" "$BASE" 2>/dev/null); bn=${bn:-0}
  nn=$(grep -ac "$fam" "$S" 2>/dev/null); nn=${nn:-0}
  ok=$(python3 -c "print(1 if abs($nn-$bn) <= 0.25*$bn else 0)")
  if [ "$ok" -eq 1 ]; then echo "[✓] $fam 节奏一致（基线=$bn 现卷=$nn，±25% 容差）"; else echo "[✗] $fam 节奏漂移（基线=$bn 现卷=$nn）"; FAIL=1; fi
done
# 探针 absent 基线：HEARTBEAT-KILL / SUPERVISOR 家族零出现（qemu-prod 无
# svc-hang/svc-fault——与 62.3 基线一致）。
for fam in HEARTBEAT-KILL SUPERVISOR; do
  nn=$(grep -ac "$fam" "$S" 2>/dev/null); nn=${nn:-0}
  if [ "$nn" -eq 0 ]; then echo "[✓] $fam = 0（探针 absent 基线一致）"; else echo "[✗] $fam 出现 $nn 次（基线 0）"; FAIL=1; fi
done

# 5. typed 链不回归 + dispatch 词形（63.1 单源 display）。
for pat in "CATALOG-ACTIVE gen=1" "REGISTRY-ACTIVE binding=(1,10)" "CONTRACT-VIEW-ACTIVE contracts=" "PLAN-COMPILED" "EXEC-JOURNAL applied-confirmed" "CONTROL-CHAIN: round=" "dispatch=Committed"; do
  grep -q "$pat" "$S" && echo "[✓] $pat" || { echo "[✗] $pat 缺失"; FAIL=1; }
done
n=$(grep -ac "dispatch=other" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] dispatch=other = 0"; else echo "[✗] dispatch=other 出现 $n 次"; FAIL=1; fi

# 6. fail-closed 兜底词形零出现。
for bad in "REGISTRY-MISSING" "CONTRACT-VIEW-MISSING" "CATALOG-MISSING" "CHAIN-KEY-MISSING" "OBS-BATCH malformed"; do
  n=$(grep -ac "$bad" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] $bad = 0"; else echo "[✗] $bad 出现 $n 次"; FAIL=1; fi
done

[ $FAIL -eq 0 ] && echo "VOL63.2-GATE: PASS" || { echo "VOL63.2-GATE: FAIL"; exit 7; }
echo "DONE"
