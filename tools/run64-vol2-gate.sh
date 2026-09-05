#!/bin/bash
# 蓝图 64 卷 64.2（v0.317.1）出口 QEMU 实弹回归（2026-09-03）：
#   出口门 3（硬门）：本卷为 target 闭包大卷（root-task + svc-modbus-tcp/
#   svc-iec104/svc-mqtt/svc-modbus-rtu 变更）——`image raw` 链重建 +
#   qemu-prod 实弹一轮。
#   判据（蓝图 [03] 出口门 3）：
#   - PRODUCT-READY: true（readiness 汇总行——Orchestration/ReadinessTracker
#     重排后词形逐字节不变）；
#   - 冻结词形存在性：PROTO-EXEC status=0x 行（drv_call 重写后）、
#     OBS-REPORT enqueue/flush rejected 行（obs_wire 解析器收编后）、
#     CONTROL-CHAIN: round= + dispatch=Committed、ACQ-ROUND、AUDIT-CHAIN、
#     OTA-DOWNLOAD、STORAGE-PERSIST、telemetry EIPC bound（NOT granted
#     词形 68.2 起退场——iec104 边接通，run63-vol2-gate 翻转同律）；
#   - 确定性行计数对照 evidence/63/vol63-1 基线：PROTOCOL-CONFIG 装载行
#     逐字节一致；POINTS-LOADED **唯一登记变更 = ingest bin 二次 fetch
#     删除 → modbus-config 每轮 2→1 行**（其余协议与基线一致）；
#   - 负断言：SUPERVISOR/HEARTBEAT-KILL 不出现（与基线一致——心跳语义
#     零变化；Orchestration 仅收拢状态面）；
#   - 驱动 15/10/5 基线：SERVICE_WIRING 表序/周期由 wiring.rs host 冻结
#     测试锁定（本 gate 不可直接观测，出口以 host 判据为准）。
#   脚本建模 tools/run64-vol1-gate.sh；证据归档 evidence/64/vol64-2/。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/64/vol64-2"
mkdir -p "$EVROOT/build"

# WP-64.3.6：build 前导单源（清场/钥导出/compile 收编 gate-env.sh）。
gate_cleanup_env 5577 5582
gate_export_signing_key


echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接 + image raw qemu-prod（root-task + 4 bin 进镜像）==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/sel4.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "SEL4-RELINK FAIL rc=$RC"; tail -20 "$EVROOT/build/sel4.log"; exit 3; }
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

echo "=== 步骤 3：qemu-prod typed 链正例一轮 ==="
DIR="$ROOT/target/evidence64-vol2/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
# peer 会把运行回放 append 进 --transcript 文件——拷基线输入到本卷证据目录
#（禁止写已跟踪的 vol62-1 基线文件；63 卷先例）。
cp -f "$ROOT/evidence/62/vol62-1/normal.modbus.transcript.jsonl" \
      "$EVROOT/normal.modbus.transcript.jsonl"
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
           "PROTO-EXEC svc-modbus-tcp status=0x00" \
           "OBS-REPORT enqueue rejected" \
           "OBS-REPORT flush rejected" \
           "OBS-REPORT svc=" \
           "CONTROL-CHAIN: round=" \
           "dispatch=Committed" \
           "ACQ-ROUND" \
           "AUDIT-CHAIN" \
           "OTA-DOWNLOAD: OK" \
           "STORAGE-PERSIST: OK" \
           "telemetry EIPC bound (svc-telemetry)"; do
  # 68.2 起无 NOT granted 词形（iec104→telemetry 边接通，缺边词形退场）。
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done
# PROTO-EXEC 状态字节词形（status_handled 收编后观测行不变；0x04=能力壳
# 服务的显式降级——qemu-prod 装配常态）。
if grep -qE "PROTO-EXEC svc-(modbus-tcp|mqtt|iec104) status=0x04" "$S"; then
  echo "[✓] PROTO-EXEC status=0x04（能力壳显式降级词形）"
else
  echo "[✗] PROTO-EXEC status=0x04 缺失"; FAIL=1
fi
# 负断言：心跳/故障重启面零触发（与基线一致）。
for pat in "SUPERVISOR" "HEARTBEAT-KILL"; do
  if grep -q "$pat" "$S"; then echo "[✗] $pat 不应出现（基线 0）"; FAIL=1; else echo "[✓] $pat 无（基线一致）"; fi
done

# 确定性行计数对照（63 vol63-1 基线；唯一登记变更 modbus POINTS-LOADED
# 2→1——二次 fetch 删除）。
python3 - "$ROOT/evidence/63/vol63-1/normal.qemu-serial.log" "$S" <<'PYEOF' || FAIL=1
import re, sys
from collections import Counter

def extract(path):
    txt = open(path, errors="replace").read()
    out = []
    for m in re.finditer(
        r"\[(\w+)-config\] (PROTOCOL-CONFIG loaded \d+ keys \(id=[\w-]+\)"
        r"|POINTS-LOADED \d+ points \(registry \d+\))",
        txt,
    ):
        out.append(f"[{m.group(1)}-config] {m.group(2)}")
    return Counter(out)

base = extract(sys.argv[1])
run = extract(sys.argv[2])
ok = True
for k in sorted(set(base) | set(run)):
    expected = base.get(k, 0)
    if "POINTS-LOADED" in k and k.startswith("[modbus-config]"):
        # 卷 64.2 唯一登记变更：modbus bin 二次 fetch 删除 → 每轮 2→1 行
        #（蓝图 [03] WP-64.2.3 / 出口门 3；行词形本身不变）。
        expected = expected - expected // 2
    b, r = base.get(k, 0), run.get(k, 0)
    if r != expected:
        print(f"[✗] 计数 基线×{b} 期望×{expected} 实跑×{r}: {k}")
        ok = False
    else:
        tag = "（登记变更 2→1）" if expected != b else ""
        print(f"[✓] ×{r}: {k} {tag}")
sys.exit(0 if ok and run else 1)
PYEOF

if [ $FAIL -ne 0 ]; then
  echo "=== VOL64.2-GATE: FAIL ==="
  exit 1
fi
echo "=== VOL64.2-GATE: PASS ==="
