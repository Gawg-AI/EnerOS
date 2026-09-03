#!/bin/bash
# 蓝图 61 v0.314.26 E4 故障矩阵：单 PCS 纵切三场景 + 0.314.24/25 既有证据复用。
#
# 矩阵行覆盖（蓝图 05 §4.5）：
#   M1 合法写 ACK+回读匹配   → AppliedConfirmed/Committed（normal 场景）
#   M2 错回读（读回偏移 1.5×）→ AppliedReadbackMismatch → Failed（wrong-readback 场景）
#   M3 transport 拒绝（无对端）→ 通道 Rejected → Indeterminate 对账（无对端场景）
#   M4 投递后崩溃/重启对账   → EXEC-JOURNAL recovered pending=N → reconciling
#                             （0.314.24 run-e4c 串口留证，本卷不复跑）
#   M5 超时                 → E3 delivery_timeout_is_indeterminate + R7
#
# 每场景独立 QEMU 窗口（600s）+ 独立证据目录；判据自动断言。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/0.314.26"
mkdir -p "$EVROOT"

export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }

# 清理前轮孤儿（peer/QEMU）——僵尸 peer 会让注入用例拿到正常回读（实测坑）。
kill -9 $(pgrep -f "modbus_slave6[1].py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-sys[t]em") 2>/dev/null || true
sleep 2

run_case() {
  local name="$1" peer_args="$2" expect_key="$3" expect_desc="$4"
  local dir="$ROOT/target/evidence61/$name"
  mkdir -p "$dir"; rm -f "$dir/qemu-serial.log" "$dir/eneros-data.raw"
  echo "=== M-$name：期望 $expect_desc ==="
  python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/$name.modbus.transcript.jsonl" $peer_args &
  local ppid=$!
  sleep 1
  # 对端存活校验（绑定失败=僵尸占端口 → 本用例证据无效）。
  if ! kill -0 $ppid 2>/dev/null; then echo "  PEER-DIED（端口被占？）M-$name: FAIL"; return 1; fi
  rm -f "$ROOT/target/evidence61/eneros-data.raw"
  cargo xtask run qemu-prod \
    --image "$IMG" \
    --vars-template "$ROOT/target/evidence61/template.fd" \
    --expect PASS --evidence-dir "$dir" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 600 >/dev/null 2>&1
  kill $ppid 2>/dev/null || true
  local log="$dir/qemu-serial.log"
  local hits=$(grep -c "$expect_key" "$log" 2>/dev/null || echo 0)
  local readback=$(grep -n "MASTER-READBACK" "$log" | head -1 | cut -d: -f1)
  local confirmed=$(grep -n "EXEC-JOURNAL applied-confirmed" "$log" | head -1 | cut -d: -f1)
  echo "  期望键「$expect_key」命中=$hits readback@$readback confirmed@$confirmed"
  cp -f "$log" "$EVROOT/$name.qemu-serial.log" 2>/dev/null || true
  [ "$hits" -ge 1 ] && echo "  M-$name: PASS" || echo "  M-$name: FAIL"
}

# M1 正常：已双跑 PASS（本轮略过，证据见 m1-normal.qemu-serial.log）
# M2 错回读：读回偏移 1.5×（5→7.5，超容差）→ AppliedReadbackMismatch → dispatch Failed
run_case "m2-wrong-readback" "--wrong-readback-scale 1.5" "readback-mismatch" "ACK 成功但物理状态未达目标 → Failed"
# M3 transport 拒绝：无对端 → driver status≠ST_OK → 通道 Rejected → Indeterminate 对账
run_case "m3-no-peer" "--port 5599" "SUBMIT state=Indeterminate" "transport 拒绝 → Indeterminate（不假失败）"

echo "=== 矩阵汇总（M4 重启对账=0.314.24 run-e4c 串口；M5 超时=E3 套件）==="
