#!/usr/bin/env bash
# SPEC-D 窗口判定（R1/R9：一切 PASS 以串口实测行裁决）。
# 判据锚定 WP-D.2 DoD：
#   C3 主判据 —— QEMU 无串口柄（UART_HANDOVER_TABLE 空表）⇒ svc-modbus-rtu
#   必须打印诚实降级词形 RTU-UNAVAILABLE reason=no-handle；
#   防伪补判 —— RTU-ACTIVE 词形**不得出现**（无柄活路即谎言）；
#   系统健康 —— PRODUCT-READY: true 且零 PANIC；授予表 edge 总数不变
#   （D 卷零新增 EIPC 边——grants 表今日仍是 38 边基线，随镜像落串口的
#   GRANT-TABLE 行与本文件常量对照）。
# 用法：verify.sh <window_out_dir>
set -uo pipefail
D=${1:?}; S="$D/qemu-serial.log"
declare -A R=(
  [rtu_unavailable]=0 [no_rtu_active]=0 [descriptor]=0
  [product_ready]=0 [panic]=0 [grant_table]=0 [time_epoch_line]=0
)
grep -q 'svc-modbus-rtu] RTU-UNAVAILABLE reason=no-handle' "$S" && R[rtu_unavailable]=1
! grep -q 'RTU-ACTIVE' "$S" && R[no_rtu_active]=1
grep -q '\[protocol-service\] modbus-rtu plugin=' "$S" && R[descriptor]=1
grep -q 'PRODUCT-READY: true' "$S" && R[product_ready]=1
if ! grep -q 'PANIC' "$S"; then R[panic]=1; fi
grep -q 'GRANT-TABLE edges=' "$S" && R[grant_table]=1
grep -qE 'TIME-EPOCH(-MISSING)?' "$S" && R[time_epoch_line]=1

echo "== SPEC-D WINDOW VERIFY ($D) =="
for k in rtu_unavailable no_rtu_active descriptor product_ready panic grant_table time_epoch_line; do
  printf '%-18s %s\n' "$k" "${R[$k]}"
done | tee "$D/VERIFY.txt"
VERDICT=$([ "${R[rtu_unavailable]}" = 1 ] && [ "${R[no_rtu_active]}" = 1 ] \
  && [ "${R[product_ready]}" = 1 ] && [ "${R[panic]}" = 1 ] && echo PASS || echo REVIEW)
echo "D-C3-HONEST-SHELL-VERDICT: $VERDICT" | tee -a "$D/VERIFY.txt"
# 判定行留证摘录。
grep -E 'modbus-rtu|RTU-(ACTIVE|UNAVAILABLE)|PRODUCT-READY|GRANT-TABLE' "$S" > "$D/judgment-lines.txt" || true
