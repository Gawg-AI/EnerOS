#!/bin/bash
# ============================================================
# 蓝图 73 批B（B-P0 / B-P3 验收）：恶意卷宗对判据路径的假绿复现与修复对照。
#
# 用法：
#   bash tools/testdata/gate-false-green/assert_false_green.sh            # 复现现行假绿（G4 的"红"）
#   bash tools/testdata/gate-false-green/assert_false_green.sh --post-fix # 收编后路径：恶意卷宗必须全 ✗
#
# 输出：
#   repro-current.txt = 现行逻辑（2026-09-22 基线 `0e4fd05` 逐字摘录）对
#   恶意 fixture 的判定——**全部 ✓ 即复现成功**（假绿物证，随规格存档）。
#   --post-fix 输出与 repro-current.txt 并排 = 修复生效的物证
#   （前 ✓ 后 ✗）。
#
# 恶意 fixture（均不含 R9 真实目标 token）：
#   r9-only-boot-identity.log = 只有 BOOT-IDENTITY-PAGE / OTA-IDENTITY /
#     BOOT-IDENTITY 等词形上偶然含 "IDENTITY" 子串的无关行；
#   r9-honest-failure.log     = 含诚实失败行 IDENTITY-PUSH: FAILED（失败窗
#     不得被判绿）；
#   b4-missing-transcript/    = 空目录（$DB 两份转写全缺 → B4 必须 红）。
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
TD="$ROOT/tools/testdata/gate-false-green"
MODE="${1:-current}"

r9_current() {  # vol5-gate R9 两行（:289-290 基线逐字）
  local SB="$1"
  n=$(grep -ac "IDENTITY" "$SB" || true); n=${n:-0}
  [ "$n" -ge 1 ] && echo "[✓][R9] 窗B 身份面观测（IDENTITY ×$n）" || { echo "[✗][R9] 窗B IDENTITY 缺失"; return 1; }
}

b4_current() {  # r10-held B4 片段（:331-353 基线逐字，含 FileNotFoundError→0）
  local T1="$1" T2="$2" S2="$3"
  N6=$(python3 - "$T1" "$T2" <<'PY' 2>/dev/null || echo "ERR"
import json, sys
def fc06(path):
    try:
        fh = open(path)
    except FileNotFoundError:
        return 0
    n = 0
    for l in fh:
        try:
            e = json.loads(l)
        except Exception:
            continue
        if e.get("func") == "fc06":
            n += 1
    return n
print(fc06(sys.argv[2]) - fc06(sys.argv[1]))
PY
)
  NDELIV=$(grep -ac "EXEC-JOURNAL applied-confirmed" "$S2" || true); NDELIV=${NDELIV:-0}
  if [ "${N6:-99}" -le "${NDELIV:-0}" ]; then
    echo "[✓][B4] 重启窗 FC06 ×$N6 ≤ journal 确认执行 ×$NDELIV（写可归属；对账只读）"
  else
    echo "[✗][B4] 重启窗 FC06 ×${N6:-?} > journal 确认执行 ×${NDELIV:-0}（存在无归属写——疑补发）"; return 1
  fi
}

r9_fixed() {  # 收编后：criterion.py check VOL70.5-R9（回声同形，判定登记化）
  python3 "$ROOT/tools/criterion.py" check VOL70.5-R9 "$1"
}

b4_fixed() {  # 收编后：fc06 缺输入 exit-3 = 红 + NDELIV 经判据器计数
  local T1="$1" T2="$2" S2="$3"
  N6=$(python3 - "$T1" "$T2" <<'PY' 2>/dev/null || echo "ERR"
import json, sys
def fc06(path):
    try:
        fh = open(path)
    except FileNotFoundError:
        sys.exit(3)  # 73 批B：缺输入=红（不再是静默 0）
    n = 0
    for l in fh:
        try:
            e = json.loads(l)
        except Exception:
            continue
        if e.get("func") == "fc06":
            n += 1
    return n
print(fc06(sys.argv[2]) - fc06(sys.argv[1]))
PY
)
  if [ "$N6" = "ERR" ]; then
    echo "[✗][B4] 重启窗转写缺失（input missing: pre-cut/modbus transcript）——缺输入=红"
    return 1
  fi
  local NDELIV RC
  NDELIV=$(python3 "$ROOT/tools/criterion.py" check R10-B4-DELIVERY --count-only "$S2" 2>/dev/null); RC=$?
  if [ "$RC" -ne 0 ]; then
    echo "[✗][B4] 重启窗 journal 输入缺失（input missing: $S2）——缺输入=红"
    return 1
  fi
  if [ "${N6:-99}" -le "${NDELIV:-0}" ]; then
    echo "[✓][B4] 重启窗 FC06 ×$N6 ≤ journal 确认执行 ×$NDELIV（写可归属；对账只读）"
  else
    echo "[✗][B4] 重启窗 FC06 ×${N6:-?} > journal 确认执行 ×${NDELIV:-0}（存在无归属写——疑补发）"; return 1
  fi
}

FAIL=0
if [ "$MODE" = "--post-fix" ]; then
  echo "== post-fix（收编后判据路径）：恶意卷宗全 ✗ 才是 PASS =="
  r9_fixed "$TD/r9-only-boot-identity.log"      || FAIL=1
  r9_fixed "$TD/r9-honest-failure.log"          || FAIL=1
  b4_fixed "$TD/b4-missing-transcript/pre-cut-transcript.jsonl" \
           "$TD/b4-missing-transcript/modbus.transcript.jsonl" \
           "$TD/b4-missing-transcript/qemu-serial.log" || FAIL=1
  if [ "$FAIL" -eq 0 ]; then
    echo "!! 假绿仍在（恶意卷宗被判 ✓）——收编失效"; exit 1
  fi
  echo "== post-fix 全 ✗ = 修复生效 =="
  exit 0
fi

echo "== current（基线逻辑逐字摘录）：全部 ✓ = 假绿复现成功 =="
OUT="$TD/repro-current.txt"
{
  echo "# 蓝图 73 批B B-P0：现行判据对恶意卷宗的假绿复现（基线 0e4fd05 逐字摘录）"
  echo "# ——本文件是物证：R9/B4 在不含真实目标 token / 缺输入的卷宗上判 ✓。"
  r9_current "$TD/r9-only-boot-identity.log"  || true
  r9_current "$TD/r9-honest-failure.log"      || true
  b4_current "$TD/b4-missing-transcript/pre-cut-transcript.jsonl" \
             "$TD/b4-missing-transcript/modbus.transcript.jsonl" \
             "$TD/b4-missing-transcript/qemu-serial.log" || true
} | tee "$OUT"
# 复现判定：三行全 ✓ 才算复现成功。
NGREEN=$(grep -c "^\[✓\]" "$OUT")
if [ "$NGREEN" -eq 3 ]; then
  echo "复现成功：3/3 恶意场景全部假绿（✓）——现行判据缺陷在案。"
  exit 0
fi
echo "复现失败（只有 $NGREEN/3 假绿）——现行逻辑与基线摘录不符，停。"
exit 1
