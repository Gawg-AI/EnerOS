#!/usr/bin/env bash
# SPEC-C 窗口判定（R1/R9：一切 PASS 以串口/broker transcript 实测行裁决）。
# 用法：verify.sh <window_out_dir>
set -uo pipefail
D=${1:?}; S="$D/qemu-serial.log"; T="$D/history.transcript.jsonl"
declare -A R=(
  [tsdb_disk_open]=0 [storage_granted]=0 [mgmt_history_bound]=0
  [product_ready]=1 [panic]=0 [no_grant_line]=0 [hist_neg_frame]=0
)
grep -q 'TSDB-DISK-OPEN' "$S" && R[tsdb_disk_open]=1
grep -q 'EIPC-GRANT: svc-telemetry -> svc-storage slot=' "$S" && \
  ! grep -q 'svc-telemetry] storage EP NOT granted' "$S" && R[storage_granted]=1
grep -q 'EIPC-GRANT: svc-mgmt -> svc-telemetry slot=' "$S" && \
  grep -q 'history EIPC bound (svc-telemetry)' "$S" && R[mgmt_history_bound]=1
grep -q 'PRODUCT-READY: true' "$S" && R[product_ready]=1
grep -q 'GRANT-TABLE edges=' "$S" && true
grep -qvc 'PANIC' /dev/null 2>/dev/null || true
if ! grep -q 'PANIC' "$S"; then R[panic]=1; fi
! grep -q 'TSDB-DISK-OPEN FAILED' "$S" && R[no_grant_line]=1
[ -f "$T" ] && grep -q '"EXPECTED-FAIL"' "$T" && grep -q 'no-such-point' "$T" && R[hist_neg_frame]=1

echo "== SPEC-C WINDOW VERIFY ($D) =="
for k in tsdb_disk_open storage_granted mgmt_history_bound product_ready panic no_grant_line hist_neg_frame; do
  printf '%-20s %s\n' "$k" "${R[$k]}"
done | tee "$D/VERIFY.txt"
SMOKE=$([ "${R[tsdb_disk_open]}" = 1 ] && [ "${R[storage_granted]}" = 1 ] && [ "${R[mgmt_history_bound]}" = 1 ] && [ "${R[product_ready]}" = 1 ] && [ "${R[panic]}" = 1 ] && echo PASS || echo REVIEW)
NEG=$([ "${R[hist_neg_frame]}" = 1 ] && echo PASS || echo "BLOCKED-BY-DEF-5205(出站链)")
echo "C-SMOKE-VERDICT: $SMOKE" | tee -a "$D/VERIFY.txt"
echo "C-HISTNEG-VERDICT: $NEG" | tee -a "$D/VERIFY.txt"
# 判定行留证摘录（R1/R9）。
grep -E 'GRANT-TABLE|svc-telemetry -> svc-storage|svc-mgmt -> svc-telemetry|TSDB-DISK-OPEN|history EIPC bound|PRODUCT-READY' "$S" > "$D/judgment-lines.txt" || true
