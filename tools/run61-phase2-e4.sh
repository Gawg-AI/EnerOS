#!/bin/bash
# 蓝图 61 Phase 2 E4 QEMU 实弹（v0.314.30 出口证据，2026-08-30）：
#   1. config compile（points.toml [[source]] 表 + 点身份块 → bundle）
#   2. image raw qemu-prod（v0.314.30 树重建：IngestionModule 深模块接入——
#      modbus 采集轮 = module.tick（周期调度/组读/准入/死区/QoS/背压）；
#      ObservationEvent = 唯一上行，legacy 58B INGEST wire 退役）
#   3. E4-P2-M1 正常用例：合法 50kW → AppliedConfirmed/Committed；
#      判据串口行 = OBS-SOURCE（配置分配 epoch）→ ACQ（事件解码，
#      同源 wire）→ OBS-ROUND sent=/accepted=（新词形）→ OBS-SHADOW
#      MATCH（影子对照保留）→ OBS-CFG loaded → PLAN-TYPED →
#      EXEC-JOURNAL applied-confirmed。
#   4. E4-P2-RESTART：同数据盘二次启动——OBS-SOURCE epoch 递增。
# 证据归档 evidence/61/phase2-e4/。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/61/phase2-e4"
mkdir -p "$EVROOT"

# WP-64.3.6：build 前导单源（清场/钥导出/compile/image 收编 gate-env.sh）。
gate_cleanup_env 5577 5582
gate_export_signing_key

if grep -q "OBS-SHADOW MISMATCH" "$S"; then echo "[✗] OBS-SHADOW MISMATCH"; FAIL=1; fi
# legacy wire 退役门：生产协议代码不再发 58B INGEST。
if grep -q "\[svc-modbus-tcp\] INGEST round=" "$S"; then
  echo "[✗] legacy INGEST wire 仍被 modbus 发送"; FAIL=1
else
  echo "[✓] legacy INGEST wire 已退役（modbus 仅 OBS 上行）"
fi
# 20 点基线一致性：ACQ 好点值与 Phase 0 基线同（400/50/50/0.6/1/0/0）。
python3 - "$S" <<'EOF'
import re, sys
vals = {}
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"ACQ point=(\d+) value=([-\d.]+) q=good", line)
    if m:
        vals.setdefault(int(m.group(1)), m.group(2))
baseline = {1: "400", 2: "50", 3: "50", 4: "0.6", 5: "1", 6: "0", 7: "0"}
bad = [(p, vals.get(p), v) for p, v in baseline.items() if vals.get(p) != v]
if bad:
    print(f"[✗] 基线值不一致: {bad}")
    sys.exit(1)
print(f"[✓] 20 点基线一致（modbus-tcp 7 点值对照 Phase 0）")
EOF
[ $? -eq 0 ] || FAIL=1

echo "=== 判据行扫描（restart epoch 对照）==="
grep -h "OBS-SOURCE" "$ROOT/target/evidence61-p2/m1-module/qemu-serial.log" \
  "$ROOT/target/evidence61-p2/restart/qemu-serial.log" 2>/dev/null | sort -u
R2="$ROOT/target/evidence61-p2/restart/qemu-serial.log"
if grep -q "OBS-SHADOW MATCH" "$R2" && ! grep -q "OBS-ROUND rejected" "$R2"; then
  echo "[✓] restart 影子轨健康"
else
  echo "[✗] restart 影子轨异常"; FAIL=1
fi
[ $FAIL -eq 0 ] && echo "E4-P2-GATE: PASS" || { echo "E4-P2-GATE: FAIL"; exit 5; }
echo "DONE"
