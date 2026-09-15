#!/bin/bash
# 蓝图 70（方向 7 / E70-F）候选证据批跑：把 70.1–70.6 的出口门顺序跑一遍，
# 每门独立日志，供 candidate 证据记录（run-record + envelope）消费。
#
# 用法：tools/run70-candidate-batch.sh [vol1 vol2 ...]（缺省 = 1..6）
# 输出：evidence/70/70.15/batch/<gate>.log（门判据日志）
#       evidence/70/70.15/batch/<gate>.exit（退出码）
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/evidence/70/70.15/batch"
mkdir -p "$OUT"

GATES=("$@")
if [ "${#GATES[@]}" -eq 0 ]; then
  GATES=(vol1 vol2 vol3 vol4 vol5 vol6)
fi

for g in "${GATES[@]}"; do
  LOG="$OUT/${g}.log"
  echo "=== [batch] ${g} start $(date -Iseconds) ===" | tee -a "$OUT/batch-progress.log"
  bash "$ROOT/tools/run70-${g}-gate.sh" >"$LOG" 2>&1
  rc=$?
  echo "$rc" > "$OUT/${g}.exit"
  echo "=== [batch] ${g} done rc=$rc $(date -Iseconds) ===" | tee -a "$OUT/batch-progress.log"
done
echo "=== [batch] ALL DONE $(date -Iseconds) ===" | tee -a "$OUT/batch-progress.log"
