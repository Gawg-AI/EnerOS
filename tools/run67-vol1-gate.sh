#!/bin/bash
# 蓝图 67 卷 67.1（v0.320.0）出口 QEMU 实弹回归（2026-09-05）：
#   接缝收编与守卫接电卷：门禁栈单源（eneros-ci 退役 + sign-keys 进 CI）/
#   chunked epoch 八站点全收编 ChunkedEndpoint + 仓走守卫 / TEL 请求 opcode
#   re-export 单源 / Solver 参数类型化 / plan 死入口收敛 / 卫生群——
#   wire 格式零变更卷（chunked epoch 字段取值恒 0→单调 = 服务端忽略值，
#   [01] §2.1 wire 中性论证），`image raw` 链重建 + qemu-prod 实弹一轮。
#   判据（[00] §4 G8 + R-67.1 epoch 词形预检）：对照 **65 卷 vol65-1 基线**：
#   - PRODUCT-READY: true；
#   - 冻结词形存在性：PROTO-EXEC status=0x 行、OBS-REPORT rejected 行、
#     CONTROL-CHAIN: round= + dispatch=Committed、ACQ-ROUND、AUDIT-CHAIN、
#     OTA-DOWNLOAD、STORAGE-PERSIST、telemetry EIPC bound（NOT granted
#     词形 68.2 起退场——iec104 边接通）；
#   - 确定性行计数对照 evidence/65/vol65-1 基线：零调整系数——本卷
#     normal 窗预期零差异；
#   - 负断言：SUPERVISOR/HEARTBEAT-KILL/DBG-EXEC 不出现；
#   - R-67.1 预检结论（开工实跑）：vol65-1 基线 serial 中 epoch 字面均为
#     他域（TIME-EPOCH 墙钟 / CAPABILITY epoch / drv-svc instance epoch /
#     source_date_epoch），chunked wire epoch 不进串口与判据行 → 判据表
#     零调整、无静默放宽。
#   脚本建模 tools/run65-vol1-gate.sh（D1 债：gate 脚本每卷新抄，收口归
#   下一卷）；证据归档 evidence/67/vol67-1/。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/67/vol67-1"
mkdir -p "$EVROOT/build"

# build 前导单源（清场/钥导出/compile 收编 gate-env.sh）。
gate_cleanup_env 5597 5602
gate_export_signing_key

echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接 + image raw qemu-prod（67.2 触 5 个 target bin）==="
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
DIR="$ROOT/target/evidence67-vol1/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
# peer 会把运行回放 append 进 --transcript 文件——拷基线输入到本卷证据目录
#（禁止写已跟踪的 vol62-1 基线文件；63/64/65 卷先例）。
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
if grep -qE "PROTO-EXEC svc-(modbus-tcp|mqtt|iec104) status=0x04" "$S"; then
  echo "[✓] PROTO-EXEC status=0x04（能力壳显式降级词形）"
else
  echo "[✗] PROTO-EXEC status=0x04 缺失"; FAIL=1
fi
for pat in "SUPERVISOR" "HEARTBEAT-KILL" "DBG-EXEC"; do
  if grep -q "$pat" "$S"; then echo "[✗] $pat 不应出现（基线 0）"; FAIL=1; else echo "[✓] $pat 无"; fi
done

# 确定性行计数对照（65 vol65-1 基线；零调整系数——本卷 wire/词形零变更，
# normal 窗预期零差异）。
python3 - "$ROOT/evidence/65/vol65-1/normal.qemu-serial.log" "$S" <<'PYEOF' || FAIL=1
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
  echo "=== VOL67-GATE: FAIL ==="
  exit 1
fi
echo "=== VOL67-GATE: PASS ==="
