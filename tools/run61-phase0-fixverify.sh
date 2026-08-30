#!/bin/bash
# 蓝图 61 Phase 0 复查修复验证（2026-08-30）：
#   1. config compile（点表加独立过温点 7 → 重编 bundle/ln）
#   2. image raw qemu-prod（0.314.26 修复树重建镜像）
#   3. E4 矩阵 M1/M2/M3 全量重跑（修复后镜像实弹）
#   4. M4 崩溃重启对账补证：无对端首轮 → 同数据盘二次启动 →
#      EXEC-JOURNAL recovered pending=N → reconciling（补 0.314.26 缺失归档）
# 证据归档 evidence/61/phase0-fixverify/（发布 0.314.26 证据目录不动）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/phase0-fixverify"
mkdir -p "$EVROOT"

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
export BINDGEN_EXTRA_CLANG_ARGS_aarch64_unknown_none="--sysroot=$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/aarch64-none-elf"
export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"
export ENEROS_RELEASE_PUBLIC_KEY_HEX="8e5150375a22ae2ad55ea85a3e7a8faee8196315a413163fd4778c82c59af9a7"
export ENEROS_RELEASE_KEY_ID_HEX="4e28c578ebd8c3c68b83ff1d0b1a16df"
export ENEROS_PLATFORM_ID_HEX="6afc39cf058a1ca52a5e36d8eefa1463"
K="$ROOT/product/security/keys-local"

# 清理前轮孤儿（peer/QEMU；含内部 socket 端口占用核验——5577 被占即秒退）。
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（points.toml 点 7 入 bundle）==="
mkdir -p "$EVROOT/build"
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --root-key-hex "$(printf '5a%.0s' {1..64})" >"$EVROOT/build/compile.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "CONFIG-COMPILE FAIL rc=$RC（RC124 容忍先例另议）"; tail -5 "$EVROOT/build/compile.log"; exit 2; }
tail -3 "$EVROOT/build/compile.log"

echo "=== 步骤 2：image raw qemu-prod（修复树重建）==="
cargo xtask image raw qemu-prod \
  --signing-key "$K/dev-signing.key" --uefi-key "$K/uefi-test.key" --uefi-cert "$K/uefi-test.pem" \
  >"$EVROOT/build/image.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "IMAGE FAIL rc=$RC"; tail -20 "$EVROOT/build/image.log"; exit 3; }
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
run_case "m3-reject-writes" "--reject-writes" "SUBMIT state=Failed" "对端拒写 → Failed 终态，不重复投递"

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
ind=$(grep -c 'SUBMIT state=Indeterminate' "$M4DIR/qemu-serial.log" 2>/dev/null); ind=${ind:-0}
echo "  首轮 SUBMIT Indeterminate=$ind"
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
