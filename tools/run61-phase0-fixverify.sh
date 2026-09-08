#!/bin/bash
# 蓝图 61 Phase 0 复查修复验证（2026-08-30）：
#   1. config compile（点表加独立过温点 7 → 重编 bundle/ln）
#   2. image raw qemu-prod（0.314.26 修复树重建镜像）
#   3. E4 矩阵 M1/M2/M3 全量重跑（修复后镜像实弹）
#   4. M4 崩溃重启对账补证：无对端首轮 → 同数据盘二次启动 →
#      EXEC-JOURNAL recovered pending=N → reconciling（补 0.314.26 缺失归档）
# 证据归档 evidence/61/phase0-fixverify/（发布 0.314.26 证据目录不动）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/phase0-fixverify"
mkdir -p "$EVROOT"

# WP-64.3.6：build 前导单源（清场/钥导出/compile/image 收编 gate-env.sh）。
gate_cleanup_env 5577 5582
gate_export_signing_key


echo "=== 步骤 1：config compile（points.toml 点 7 入 bundle）==="
mkdir -p "$EVROOT/build"
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：image raw qemu-prod（修复树重建）==="
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

run_case() {
  local name="$1" peer_args="$2" expect_key="$3" expect_desc="$4"
  local dir="$ROOT/target/evidence61-fixverify/$name"
  mkdir -p "$dir"; rm -f "$dir/qemu-serial.log" "$dir/eneros-data.raw"
  echo "=== FV-$name：期望 $expect_desc ==="
  python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/$name.modbus.transcript.jsonl" $peer_args &
  local ppid=$!
  sleep 1
  if ! kill -0 $ppid 2>/dev/null; then echo "  PEER-DIED F-$name: FAIL"; return 1; fi
  cargo xtask run qemu-prod \
    --image "$IMG" \
    --vars-template "$ROOT/target/evidence61/template.fd" \
    --expect PASS --evidence-dir "$dir" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 600 >/dev/null 2>&1
  kill $ppid 2>/dev/null || true
  local log="$dir/qemu-serial.log"
  local hits; hits=$(grep -c "$expect_key" "$log" 2>/dev/null); hits=${hits:-0}
  echo "  期望键「$expect_key」命中=$hits"
  cp -f "$log" "$EVROOT/$name.qemu-serial.log" 2>/dev/null || true
  [ "$hits" -ge 1 ] && echo "  FV-$name: PASS" || echo "  FV-$name: FAIL"
}

run_case "m1-normal" "" "applied-confirmed" "合法写 ACK+回读匹配 → Committed"
run_case "m2-wrong-readback" "--wrong-readback-scale 1.5" "readback-mismatch" "错回读 1.5× → Failed 零 Committed"
# M3（复查矫正场景）：peer 在位供遥测、但拒写（--reject-writes）——
# 「transport 拒绝」须在反馈健康前提下抵达写路径；旧「无对端」在零合成
# 源的正确装配下 = 无遥测 = 链不提交（陈旧 root 合成源掩盖的历史偶然）。
# 对端协议级拒写 → writer Denied(8) → dispatch Failed 终态（蓝图
# 05 §4.5「transport 拒绝 | Failed」行）。
# （69 全卷独立复查：词形前缀随 v0.329.0 旧读臂退役更新 SUBMIT→TYPED）
run_case "m3-reject-writes" "--reject-writes" "TYPED state=Failed" "对端拒写 → Failed 终态，不重复投递"

echo "=== 步骤 4：M4 崩溃重启对账补证（读回不可得 → Indeterminate + 同数据盘二次启动）==="
M4DIR="$ROOT/target/evidence61-fixverify/m4-restart-reconcile"
mkdir -p "$M4DIR"; rm -f "$M4DIR/qemu-serial.log" "$M4DIR/eneros-data.raw"
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/m4-restart-reconcile.modbus.transcript.jsonl" --fail-readback &
M4PEER=$!
sleep 1
# 首轮：遥测健康、写 ACK、读回不可得 → AppliedReadbackUnavailable →
# Indeterminate 非终态持久化（蓝图 05 §4.5「读回不可得 | Indeterminate」行）。
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$ROOT/target/evidence61/template.fd" \
  --expect PASS --evidence-dir "$M4DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 600 >/dev/null 2>&1
ind=$(grep -c 'TYPED state=Indeterminate' "$M4DIR/qemu-serial.log" 2>/dev/null); ind=${ind:-0}
echo "  首轮 TYPED Indeterminate=$ind"
# 二轮：同 evidence-dir（数据盘复用，不清盘）→ 启动恢复对账。
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$ROOT/target/evidence61/template.fd" \
  --expect PASS --evidence-dir "$M4DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs 600 >/dev/null 2>&1
kill $M4PEER 2>/dev/null || true
RECOVERED=$(grep -c 'EXEC-JOURNAL recovered pending=' "$M4DIR/qemu-serial.log" 2>/dev/null); RECOVERED=${RECOVERED:-0}
grep -m1 'EXEC-JOURNAL recovered pending=' "$M4DIR/qemu-serial.log" 2>/dev/null || true
cp -f "$M4DIR/qemu-serial.log" "$EVROOT/m4-restart-reconcile.qemu-serial.log" 2>/dev/null || true
[ "$RECOVERED" -ge 1 ] && echo "  FV-m4-restart-reconcile: PASS（recovered 行在串口）" || echo "  FV-m4-restart-reconcile: FAIL"

echo "=== fixverify 完成（证据在 $EVROOT）==="
