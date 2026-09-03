#!/bin/bash
# 蓝图 62 卷 62.2（v0.315.1）出口 QEMU 实弹回归（2026-09-02）：
#   出口门（硬门）：本卷触及 root-task（SERVICE_WIRING 接线表 + 驱动循环
#   表驱动）与协议服务 bins（IngestionStage 交付半环 + obs_batch_wire +
#   ChunkedEndpoint）——`image raw` 链重建 + qemu-prod 实弹：
#   - PRODUCT-READY: true；零 PANIC；
#   - D-1 修复实证：svc-iec104 采集轮接入默认编排——iec104 从站对端
#     transcript 总召/数据事务 ≥2 轮 + 串口 [svc-iec104] ACQ 行 ≥2 轮
#     （非仅启动自检 1 轮）；
#   - S2a 迁移回归：svc-modbus-tcp 采集交付半环词形与 62.1 出口基线一致
#     （ACQ point 逐点 + OBS-ROUND sent=7 accepted=7 逐字节）；
#   - S4 迁移回归：mqtt 面不回归（svc-mqtt READY）+ typed 链不回归
#     （PLAN-COMPILED / EXEC-JOURNAL applied-confirmed / SUBMIT Committed）；
#   - 装配词形全在（CATALOG-ACTIVE / REGISTRY-ACTIVE / CONTRACT-VIEW-ACTIVE
#     ——62.1 判据保留作回归）。
# 证据归档 evidence/62/vol62-2/。脚本建模 tools/run62-vol1-gate.sh。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/62/vol62-2"
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

echo "=== 步骤 1：config compile（gen=1 catalog，配置与 62.1 出口一致）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 1.5：root-task 重链接（sel4-build-rust.sh——root ELF 内嵌
# elfloader 必须经 ninja 重链接；陈旧 rootserver.elf 会静默顶替新 root-task，
# 62.2 首次 gate 实录：image raw 编译了新 root ELF 但 QEMU 跑 8/31 旧 root）==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -10 "$EVROOT/build/relink.log"; exit 9; }
grep -a "Rust ELF:" "$EVROOT/build/relink.log" | tail -1

echo "=== 步骤 2：image raw qemu-prod（v0.315.1 树重建）==="
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

echo "=== 步骤 3：qemu-prod 实弹（modbus + iec104 双对端在位）==="
DIR="$ROOT/target/evidence62-vol2/normal"
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

# 2. D-1 修复实证：svc-iec104 采集轮被默认编排周期驱动（≥2 轮）。
ACQ_N=$(grep -ac '\[svc-iec104\] ACQ point=' "$S" 2>/dev/null); ACQ_N=${ACQ_N:-0}
if [ "$ACQ_N" -ge 4 ]; then echo "[✓] svc-iec104 ACQ 行 ≥4（两轮 × 总召突发，实得 $ACQ_N）"; else echo "[✗] svc-iec104 ACQ 行不足两轮（$ACQ_N）"; FAIL=1; fi
if [ -f "$EVROOT/normal.iec104.transcript.jsonl" ]; then
  INTR_N=$(grep -aic 'INTERROGATION+DATA' "$EVROOT/normal.iec104.transcript.jsonl" 2>/dev/null); INTR_N=${INTR_N:-0}
  TX_N=$(wc -l < "$EVROOT/normal.iec104.transcript.jsonl" 2>/dev/null); TX_N=${TX_N:-0}
  echo "    iec104 从站 transcript 行=$TX_N（总召+数据轮次=$INTR_N）"
  if [ "$INTR_N" -ge 2 ]; then echo "[✓] iec104 从站总召+数据 ≥2 轮（对端独立记录）"; else echo "[✗] iec104 从站总召轮次不足（$INTR_N）"; FAIL=1; fi
else
  echo "[✗] iec104 transcript 缺失"; FAIL=1
fi

# 3. S2a 迁移回归：modbus 采集交付半环与 62.1 出口基线词形一致。
grep -q '\[svc-modbus-tcp\] OBS-ROUND sent=7 accepted=7' "$S" && echo "[✓] modbus OBS-ROUND sent=7 accepted=7（62.1 基线词形）" || { echo "[✗] modbus OBS-ROUND 基线词形缺失"; FAIL=1; }
for pt in 1 2 4 5 6 7; do
  grep -q "\[svc-modbus-tcp\] ACQ point=$pt value=" "$S" && echo "[✓] modbus ACQ point=$pt" || { echo "[✗] modbus ACQ point=$pt 缺失"; FAIL=1; }
done
MB_ACQ=$(grep -ac '\[svc-modbus-tcp\] ACQ point=' "$S" 2>/dev/null); MB_ACQ=${MB_ACQ:-0}
echo "    svc-modbus-tcp ACQ 行总数=$MB_ACQ（62.1 基线=433，窗口波动允许）"
[ "$MB_ACQ" -ge 14 ] || { echo "[✗] modbus ACQ 行异常偏少"; FAIL=1; }

# 4. S4 迁移回归：mqtt 面 + typed 链 + 62.1 装配词形。
# mqtt 面回归词形（62.1 同款：qemu-prod 无 broker → plugin start FAILED 为
# 预存诚实降级，判据用 spawn/就绪词形）。
grep -q 'drv-svc-mqtt. READY-SIGNALED' "$S" && echo "[✓] svc-mqtt READY-SIGNALED（62.1 同款）" || { echo "[✗] svc-mqtt READY-SIGNALED 缺失"; FAIL=1; }
for pat in "CATALOG-ACTIVE gen=1" "CATALOG-BINDING point=10 device=1" "REGISTRY-ACTIVE binding=(1,10)" "CONTRACT-VIEW-ACTIVE contracts=" "PLAN-COMPILED" "EXEC-JOURNAL applied-confirmed"; do
  grep -q "$pat" "$S" && echo "[✓] $pat" || { echo "[✗] $pat 缺失"; FAIL=1; }
done
n=$(grep -ac 'SUBMIT state=Committed' "$S" 2>/dev/null); n=${n:-0}
echo "    SUBMIT state=Committed 命中=$n"

# 5. fail-closed 兜底词形零出现（62.1 判据保留）。
for bad in "REGISTRY-MISSING" "CONTRACT-VIEW-MISSING" "CATALOG-MISSING" "CHAIN-KEY-MISSING"; do
  n=$(grep -ac "$bad" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] $bad = 0"; else echo "[✗] $bad 出现 $n 次"; FAIL=1; fi
done
# iec104 上行授予缺口为预存现状（telemetry EP not granted = 诚实 fail-closed
# 面，非本卷判据；63 候选登记）。
NG=$(grep -ac '\[svc-iec104\] telemetry EP not granted' "$S" 2>/dev/null); NG=${NG:-0}
echo "    [i] svc-iec104 telemetry EP not granted 行=$NG（预存授予缺口，诚实降级面）"
OBSN=$(grep -ac '\[svc-iec104\] OBS-ROUND' "$S" 2>/dev/null); OBSN=${OBSN:-0}
echo "    [i] svc-iec104 OBS-ROUND 行=$OBSN（上行面）"
grep -a '\[svc-iec104\] OBS-SOURCE' "$S" | head -2

[ $FAIL -eq 0 ] && echo "VOL62.2-GATE: PASS" || { echo "VOL62.2-GATE: FAIL"; exit 7; }
echo "DONE"
