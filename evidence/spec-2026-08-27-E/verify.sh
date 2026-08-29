#!/usr/bin/env bash
# SPEC-E 窗口判定行核验（对齐卷文档 §5 DoD 与执行状态表）。
# 用法：verify.sh <window_dir>
set -uo pipefail
D="$1"; S="$D/qemu-serial.log"
pass=0; fail=0
chk() { # chk <name> <grep-pattern> <expect:min-count>
  local n; n=$(grep -c "$2" "$S" 2>/dev/null || echo 0)
  if [ "${3:-1}" = "0" ]; then
    if [ "$n" = "0" ]; then echo "PASS  $1 (count=0 as required)"; pass=$((pass+1));
    else echo "FAIL  $1 (count=$n, expected 0)"; fail=$((fail+1)); fi
  else
    if [ "$n" -ge "$3" ]; then echo "PASS  $1 (count=$n)"; pass=$((pass+1));
    else echo "FAIL  $1 (count=$n < $3)"; fail=$((fail+1)); fi
  fi
}
echo "== SPEC-E window: $D =="
chk "E.2 审计链真持久装配（fresh/recovered/tail-trimmed）" "AUDIT-CHAIN-DURABLE" 1
chk "E.2 篡改负例（如该窗为负例窗）" "AUDIT-VERIFY-FAIL" "${REQ_VERIFY_FAIL:-0}"
chk "E.3 学习器装配（LEARN-STATE）" "LEARN-STATE" 1
chk "E.3 阈值单源判定（LEARN-THRESHOLDS）" "LEARN-THRESHOLDS" 1
chk "E.3 学习生效序列（LEARN-APPLY）" "LEARN-APPLY" "${REQ_APPLY:-1}"
chk "E.3 越界拒绝（LEARN-REJECT-CAP，可选）" "LEARN-REJECT-CAP" "${REQ_CAP:-0}"
chk "E.1 围栏对端采信（used=true）" "used=true" "${REQ_USED:-0}"
chk "E.1 漂移拒绝（verdict=Bad）" "verdict=Bad" "${REQ_BAD:-0}"
echo "== pass=$pass fail=$fail =="
[ "$fail" = "0" ]
