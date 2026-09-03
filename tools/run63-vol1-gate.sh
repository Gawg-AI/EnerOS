#!/bin/bash
# 蓝图 63 卷 63.1（v0.316.0）出口 QEMU 实弹回归（2026-09-03）：
#   出口门 4（硬门）：本卷触及 root-task 显示收窄（DispatchState 码表单源
#   decode/display_name 替代内联 4 态）+ svc-control-dispatch 观测面 getter
#   （AUDIT-CHAIN 行计数来源）+ engine 结算样板收拢（ExecutionCtx）——
#   `image raw` 链重建 + qemu-prod typed 链正例一轮：
#   - PRODUCT-READY: true；
#   - CONTROL-CHAIN 终态词形逐字节不变（round=… dispatch=Committed 由单源
#     display_name 供给；dispatch=- 保持——推进态/未知码如实 "-"，零 "other"）；
#   - EXEC-JOURNAL applied-confirmed / PLAN-COMPILED 词形在（63.1.3 收拢后
#     target 实弹）；
#   - AUDIT-CHAIN appended/dropped 计数行（63.1.2 getter 读数，词形不变）。
# 证据归档 evidence/62/vol62-1 基线对照 + evidence/63/vol63-1/。脚本建模
# tools/run62-vol1-gate.sh。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/63/vol63-1"
mkdir -p "$EVROOT/build"

# WP-64.3.6：build 前导单源（清场/钥导出/compile/image 三键参块收编
# gate-env.sh；compile 与 image 之间需 sel4 重链接 → 分相调用）。
gate_cleanup_env 5577 5582
gate_export_signing_key

echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接 + image raw qemu-prod（root-task 进镜像）==="
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
DIR="$ROOT/target/evidence63-vol1/normal"
mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
# peer 会把运行回放 append 进 --transcript 文件——必须先拷基线输入到本卷
# 证据目录（禁止写已跟踪的 vol62-1 基线文件）。
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
BASE="$ROOT/evidence/62/vol62-1/normal.qemu-serial.log"
# 词形逐字节不变（对照 evidence/62/vol62-1 基线）：终态由单源 display_name
# 供给；AUDIT-CHAIN 行按基线口径（typed 链正例基线只含 AUDIT-CHAIN-DURABLE
# ——AUDIT-CHAIN appended= 是协议写意图路径 DISP_SUBMIT_INTENT 的行，基线无，
# 本门也不要求）。
for pat in "PRODUCT-READY: true" \
           "CONTROL-CHAIN: round=" \
           "dispatch=Committed" \
           "PLAN-COMPILED" \
           "EXEC-JOURNAL applied-confirmed"; do
  if grep -q "$pat" "$S"; then echo "[✓] $pat"; else echo "[✗] $pat 缺失"; FAIL=1; fi
done
# root 收窄负判据：原内联 match 的 "other" 词形不得出现（终态才显示）。
n=$(grep -c "dispatch=other" "$S" 2>/dev/null); n=${n:-0}
if [ "$n" -eq 0 ]; then echo "[✓] dispatch=other = 0（root 显示收窄）"; else echo "[✗] dispatch=other 出现 $n 次（收窄失败）"; FAIL=1; fi
# AUDIT-CHAIN 行与基线逐字节一致（63.1.2 getter 读数面——typed 链下基线
# 与现卷都只出 AUDIT-CHAIN-DURABLE；词形漂移即红）。
BASE_AUDIT=$(grep "AUDIT-CHAIN" "$BASE" | head -1)
NOW_AUDIT=$(grep "AUDIT-CHAIN" "$S" | head -1)
if [ "$BASE_AUDIT" = "$NOW_AUDIT" ] && [ -n "$NOW_AUDIT" ]; then
  echo "[✓] AUDIT-CHAIN 行与基线逐字节一致"
else
  echo "[✗] AUDIT-CHAIN 行漂移：基线='$BASE_AUDIT' 现='$NOW_AUDIT'"; FAIL=1
fi
# CONTROL-CHAIN dispatch 终态词形与基线一致（Committed 出现 + 无其他词形漂移）。
for w in "dispatch=Committed" "dispatch=-"; do
  bn=$(grep -c "$w" "$BASE" 2>/dev/null); bn=${bn:-0}
  nn=$(grep -c "$w" "$S" 2>/dev/null); nn=${nn:-0}
  echo "    $w 基线=$bn 现卷=$nn"
  if [ "$bn" -eq 0 ] && [ "$nn" -ne 0 ]; then echo "[✗] $w 基线零出现但现卷出现（词形新增）"; FAIL=1; fi
done

[ $FAIL -eq 0 ] && echo "VOL63.1-GATE: PASS" || { echo "VOL63.1-GATE: FAIL"; exit 7; }
echo "DONE"
