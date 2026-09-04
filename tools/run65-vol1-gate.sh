#!/bin/bash
# 蓝图 65 卷 65.1（v0.318.0）出口 QEMU 实弹回归（2026-09-05）：
#   出口门 4（硬门）：本卷 keyring（config-bundle 路径）/decision-contract/
#   planner/certifier/control-dispatch/agent 等均涉 target 闭包——
#   不变式归位卷：keyring tombstone（唯一授权深模块语义变更，消费面逐
#   case 等价）/sm2 守卫全仓化（7 站迁移 + 2 站 EXEMPT_FILES）/Parts
#   构造面（wire 字节零变化）/required 进叶/卫生收口——行为不变收口卷，
#   `image raw` 链重建 + qemu-prod 实弹一轮。
#   判据（蓝图 [00] §3.4 冻结词形 + [02] 出口门 4）：对照 **64 卷 vol64-3
#   基线**（64.2 的 POINTS-LOADED 2→1 偏移已烘焙进该基线）：
#   - PRODUCT-READY: true；
#   - 冻结词形存在性：PROTO-EXEC status=0x 行、OBS-REPORT rejected 行、
#     CONTROL-CHAIN: round= + dispatch=Committed、ACQ-ROUND、AUDIT-CHAIN、
#     OTA-DOWNLOAD、STORAGE-PERSIST、telemetry EIPC bound/NOT-granted；
#   - 确定性行计数对照 evidence/64/vol64-3 基线：零调整系数（64.2 偏移
#     已在基线内）——本卷唯一授权词形变更 = DBG-EXEC 行删除，但该行在
#     normal 窗本就不可达（64.3 基线计数=0，仅未识别 EXECUTE 载荷触
#     发），故 normal 窗预期**零差异**；
#   - 负断言：SUPERVISOR/HEARTBEAT-KILL 不出现；**DBG-EXEC 不出现**（唯
#     一授权删除的唯一实弹面）。
#   脚本建模 tools/run64-vol3-gate.sh；证据归档 evidence/65/vol65-1/。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/65/vol65-1"
mkdir -p "$EVROOT/build"

# build 前导单源（清场/钥导出/compile 收编 gate-env.sh）。
gate_cleanup_env 5597 5602
gate_export_signing_key

echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接 + image raw qemu-prod（keyring/decision-contract/planner/certifier 路径全涉闭包）==="
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
DIR="$ROOT/target/evidence65-vol1/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
# peer 会把运行回放 append 进 --transcript 文件——拷基线输入到本卷证据目录
#（禁止写已跟踪的 vol62-1 基线文件；63/64 卷先例）。
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
           "telemetry EIPC bound (svc-telemetry)" \
           "telemetry EIPC NOT granted"; do
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done
if grep -qE "PROTO-EXEC svc-(modbus-tcp|mqtt|iec104) status=0x04" "$S"; then
  echo "[✓] PROTO-EXEC status=0x04（能力壳显式降级词形）"
else
  echo "[✗] PROTO-EXEC status=0x04 缺失"; FAIL=1
fi
for pat in "SUPERVISOR" "HEARTBEAT-KILL" "DBG-EXEC"; do
  if grep -q "$pat" "$S"; then echo "[✗] $pat 不应出现（基线 0 / R-65.g 删除面）"; FAIL=1; else echo "[✓] $pat 无"; fi
done

# 确定性行计数对照（64 vol64-3 基线；零调整系数——64.2 POINTS-LOADED
# 2→1 偏移已烘焙进基线；65.1 normal 窗预期零差异）。
python3 - "$ROOT/evidence/64/vol64-3/normal.qemu-serial.log" "$S" <<'PYEOF' || FAIL=1
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
    b, r = base.get(k, 0), run.get(k, 0)
    if r != b:
        print(f"[✗] 计数 基线×{b} 实跑×{r}: {k}")
        ok = False
    else:
        print(f"[✓] ×{r}: {k}")
sys.exit(0 if ok and run else 1)
PYEOF

if [ $FAIL -ne 0 ]; then
  echo "=== VOL65-GATE: FAIL ==="
  exit 1
fi
echo "=== VOL65-GATE: PASS ==="
