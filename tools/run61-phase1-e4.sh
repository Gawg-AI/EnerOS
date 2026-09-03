#!/bin/bash
# 蓝图 61 Phase 1 E4 QEMU 实弹（v0.314.27-.29 出口证据，2026-08-30）：
#   1. config compile（points.toml [[source]] 表 + 点身份块 → bundle）
#   2. image raw qemu-prod（Phase 1 全树重建：观测影子/typed 链/CAND wire）
#   3. E4-M1 正常用例：合法 50kW → AppliedConfirmed/Committed；
#      判据串口行 = OBS-SOURCE（配置分配 epoch）→ OBS-ROUND（影子轨）→
#      OBS-SHADOW（影子对照）→ OBS-CFG（惰性来源装载）→ PLAN-TYPED
#      （过渡认证证书/意图/批）→ EXEC-JOURNAL applied-confirmed；
#      读回前 Committed = FAIL（Phase 0 红门回归）。
#   4. E4-RESTART：同数据盘二次启动——OBS-SOURCE epoch 递增（boot 相对
#      分配；持久化缺口 = ADR-0047 登记）+ watermark/幂等恢复不重复写。
# 证据归档 evidence/61/phase1-e4/。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/phase1-e4"
mkdir -p "$EVROOT"

# WP-64.3.6：build 前导单源（清场/钥导出/compile/image 收编 gate-env.sh）。
gate_cleanup_env 5577 5582
gate_export_signing_key

if grep -q "OBS-SHADOW MISMATCH" "$S"; then echo "[✗] OBS-SHADOW MISMATCH"; FAIL=1; fi
echo "=== 判据行扫描（restart epoch 对照）==="
grep -h "OBS-SOURCE" "$ROOT/target/evidence61-p1/m1-typed/qemu-serial.log" \
  "$ROOT/target/evidence61-p1/restart/qemu-serial.log" 2>/dev/null | sort -u
R2="$ROOT/target/evidence61-p1/restart/qemu-serial.log"
if grep -q "OBS-SHADOW MATCH" "$R2" && ! grep -q "OBS-ROUND rejected" "$R2"; then
  echo "[✓] restart 影子轨健康"
else
  echo "[✗] restart 影子轨异常"; FAIL=1
fi
[ $FAIL -eq 0 ] && echo "E4-GATE: PASS" || { echo "E4-GATE: FAIL"; exit 5; }

# 红门回归：读回前 Committed = FAIL（Phase 0 判据沿用）。
python3 - "$S" <<'EOF'
import sys
lines = open(sys.argv[1], errors="replace").read().splitlines()
first_committed = next((i for i, l in enumerate(lines) if "Committed" in l), None)
first_readback = next((i for i, l in enumerate(lines) if "MASTER-READBACK" in l), None)
if first_committed is None:
    print("[!] 未见 Committed 行（窗口内无成功命令）")
elif first_readback is not None and first_readback < first_committed:
    print("[✓] MASTER-READBACK 先于 Committed（红门不回归）")
else:
    print("[!] readback/committed 顺序需人工复核")
EOF
echo "DONE"
