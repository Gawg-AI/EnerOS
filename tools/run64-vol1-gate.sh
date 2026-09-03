#!/bin/bash
# 蓝图 64 卷 64.1（v0.317.0）出口 QEMU 实弹回归（2026-09-03）：
#   出口门 3（硬门）：本卷触及 config-bundle protocol.rs（键表单源委托
#   eneros-protocol-keys）——config-bundle 经 root default-features=false
#   进 target 闭包，`image raw` 链重建 + qemu-prod 实弹一轮。
#   判据（蓝图 [02] 出口门 3——"PROTOCOL-KV apply/NoOp 行、POINTS-LOADED
#   装载行与 63 卷串口基线逐字节一致"；实际串口词形 = 每协议
#   `[<proto>-config] PROTOCOL-CONFIG loaded N keys (id=...)` 装载行 +
#   `[<svc>-config] POINTS-LOADED N points (registry M)` 行）：
#   - PRODUCT-READY: true；
#   - 全部协议配置装载行与 evidence/63/vol63-1 基线**逐字节一致**
#     （含键计数——键表收口零行为变化的实弹证明；modbus POINTS-LOADED
#     双打维持 = 二次 fetch 现状，64.2 才收）。
#   脚本建模 tools/run63-vol1-gate.sh；证据归档 evidence/64/vol64-1/。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/64/vol64-1"
mkdir -p "$EVROOT/build"

export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
K="$ROOT/product/security/keys-local"

kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（qemu-prod）==="
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --root-key-hex "$(printf '5a%.0s' {1..64})" >"$EVROOT/build/compile.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "CONFIG-COMPILE FAIL rc=$RC"; tail -5 "$EVROOT/build/compile.log"; exit 2; }
tail -2 "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接 + image raw qemu-prod（root-task 进镜像）==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/sel4.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "SEL4-RELINK FAIL rc=$RC"; tail -20 "$EVROOT/build/sel4.log"; exit 3; }
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

echo "=== 步骤 3：qemu-prod typed 链正例一轮 ==="
DIR="$ROOT/target/evidence64-vol1/normal"
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
           "CONTROL-CHAIN: round=" \
           "dispatch=Committed"; do
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done

# 装载行清单与 63 卷基线逐字节一致（Python 抽取器两边同源，消除串口
# 交错噪声；计数含出现次数）。
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
    b, r = base.get(k, 0), run.get(k, 0)
    if b != r:
        print(f"[✗] 计数漂移 基线×{b} 实跑×{r}: {k}")
        ok = False
    else:
        print(f"[✓] ×{r}: {k}")
sys.exit(0 if ok and run else 1)
PYEOF

if [ $FAIL -ne 0 ]; then
  echo "=== VOL64.1-GATE: FAIL ==="
  exit 1
fi
echo "=== VOL64.1-GATE: PASS ==="
