#!/bin/bash
# ============================================================
# F70-07（R12 / evidence42-candidate-check 门）：候选判定库红测门。
#
# 判据：xtask 判定库全量测试绿（含 F70-01 迁入树内的 review70_* 对抗
# 红测——旧实现上必红、修后全绿），且红测确在运行（--list 可见）。
# 退出：FAIL=0 → "candidate-check red tests: PASS"（供候选记录器按
# gate=evidence42-candidate-check 归档）。
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

OUT=$(cargo test -p eneros-xtask --locked evidence_chain 2>&1)
RC=$?
echo "$OUT"

# 红测必须在场并在跑（--list ≥2 项 review70_*——防止"测试被删也绿"）。
LIST=$(cargo test -p eneros-xtask --locked evidence_chain -- --list 2>/dev/null | grep -c "review70_")

if [ $RC -eq 0 ] && [ "$LIST" -ge 2 ]; then
  echo "candidate-check red tests: PASS (review70_* x${LIST})"
  exit 0
fi
echo "candidate-check red tests: FAIL (rc=$RC listed=$LIST)"
exit 1
