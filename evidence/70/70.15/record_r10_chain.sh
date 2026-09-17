#!/bin/bash
# 蓝图 70 R10 会话：候选链 12 case 全量录制驱动（门全绿后执行）。
# 录制参数逐 case 复刻 8355d2d4 链（run-record + envelope 字段同源）。
# R10 = 真实专属门（tools/run70-r10-held.sh，2026-09-17 建立）：三窗
# 实弹（健康多时段 / 脏断电重启续接 / 0x201 按址拒写部分失败），逐意图
# 判据取代 assets=2/DEFERRED 伪证配方。
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
REC="tools/run70-candidate-evidence.py"
B="evidence/70/70.15/batch"

rec() { # case gate log command scope profile artifact ran_sha verdict_line oracle positive_id positive_detail negative_id negative_detail
  local artifact_name="$6"
  [[ "$7" != *eneros-combined.raw ]] || artifact_name="qemu-test-combined"
  python3 "$REC" record \
    --case "$1" --gate "$2" --log "$B/$3" --command "$4" --scope "$5" \
    --profile "$6" --artifact "$7" --artifact-name "$artifact_name" --ran-artifact-sha-file "$8" \
    --verdict-line "$9" \
    --oracle "${10}" \
    --positive "${11}:${12}" \
    --negative "${13}:${14}" \
    || return $?
  echo "[rec] $1 OK"
}

rec CORE-70-R01 VOL70.1 vol1.log \
  "tools/run70-vol1-gate.sh（DL=1300）" L2 qemu-prod \
  target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  evidence/70/70.1/image.sha256 \
  "=== VOL70.1 QEMU WINDOW: PASS ===" \
  "judge G1-G7 + 生产预算审计 10000 次 host 满载测试" \
  survival "PASS:G1-G7 全 ✓；审计满载仅 host scope" skip-trial "FAIL:注入 PANIC/不足提交/尾部无产出由 judge 自检判负"

rec CORE-70-R02 VOL70.2 vol2.log \
  "tools/run70-vol2-gate.sh（SEED_DL=420/REUSE_DL=1200）" L2 qemu-prod \
  target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  evidence/70/70.2/image.sha256 \
  "=== VOL70.2 QEMU WINDOW: PASS ===" \
  "段2 复用盘 + judge G1-G7" \
  reuse "PASS:全门 rc=0 ✓" none "FAIL:无独立负例判据"

rec CORE-70-R03 VOL70.2-R03 r03-powercut.log \
  "tools/run70-r03-powercut.sh（脏断电×2）" L2 qemu-prod \
  target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  evidence/70/70.2/powercut/image.sha256 \
  "[✓][R2] 脏断电各窗零 PANIC" \
  "掉电恢复合同" \
  power-cut "PASS:points>0 ✓、零 PANIC ✓、PRODUCT-READY 逐次 ✓、checkpoint 恢复 ✓" none "FAIL:无独立负例判据"

rec CORE-70-R04 VOL70.3 vol3.log \
  "tools/run70-vol3-gate.sh（DL=1200）" L2 qemu-prod \
  target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  evidence/70/70.3/image.sha256 \
  "=== VOL70.3 GATE: PASS ===" \
  "IPC 有界组装/判别 + judge G1-G7" \
  ipc "PASS:全门 rc=0 ✓" none "FAIL:无独立负例判据"

rec CORE-70-R05 VOL70.4 vol4.log \
  "tools/run70-vol4-gate.sh（DL=1000；qemu-test 装配 + 段1 240s + 段5 长断链 420s——synth 节奏适配后 miss 攒满须覆盖 3 轮换）" INTEROP qemu-test \
  target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  evidence/70/70.4/image.sha256 \
  "[✓][R1] 零 PANIC / 零分配失败" \
  "broker/peer 故障恢复：段1 健康链 used=true、锚1 后新连接+订阅+授权进展、长断链 LEASE-ARMED+降级可观测、锚2 后重连+订阅恢复" \
  recovery "PASS:R1 零 PANIC ✓、R2 段1 used=true ✓、恢复面全 ✓" skip "FAIL:R4 peer 重启后零应答=长断链武装后的安全设计（AI 请求停发；seq 跨重启判据由段0 host 窗承载）"

rec CORE-70-R06 VOL70.5 vol5.log \
  "tools/run70-vol5-gate.sh（DL=560 窗A + 窗B）" L3 qemu-prod \
  target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  evidence/70/70.5/image-a.sha256 \
  "[✓][R7] 零 PANIC / 零分配失败" \
  "生产身份面 16 项 + R6 注入交叉核验" \
  identity "PASS:16/16 全 ✓" skip-trial "FAIL:R5b 探针不在 release 装配 = 隔离负例判据"

rec CORE-70-R07 VOL70.6 vol6.log \
  "tools/run70-vol6-gate.sh（svc-config 写后读回生效后）" L2 qemu-test \
  target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  evidence/70/70.6/image-t.sha256 \
  "[✓][R1] 零 PANIC / 零分配失败" \
  "管理动作全链 22 判据" \
  mgmt "PASS:零 ✗ 全 ✓" skip "FAIL:R17 维护外激活=宿主闸覆盖（如实登记）"

rec CORE-70-R08 VOL70.7 vol7-evidence.log \
  "tools/run70-vol7-gate.sh（DL=700/DL_T2=420）" L3 qemu-test \
  target/evidence70-vol7/eneros-combined.raw \
  evidence/70/70.7/combined.sha256 \
  "VOL70.7-GATE: PASS" \
  "双镜像 A/B；BOOT-TRIAL→跨重启→BOOT-CONFIRM；R1-R10" \
  boot "PASS:R1-R10 全 ✓" skip-trial "FAIL:无 trial 直接确认被拒"

R89_ORACLE="R09 负例面 + R10 多资产：v2 链激活（assets=2）+ 四类负例 fail-closed + 恢复段 used=true + 零 PANIC"
rec CORE-70-R09 VOL70.89 vol89.log \
  "tools/run70-vol89-gate.sh（--negative-cycle + R10 多资产目录生效）" L3 qemu-test \
  target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  evidence/70/70.89/image.sha256 \
  "[✓][R1] 结构化 v2 请求到达 peer" \
  "$R89_ORACLE" \
  activation "PASS:R1 ✓、R1b used=true ✓、R2-R5 负例词形 ✓、R6 恢复 ✓、R7 零 PANIC ✓" skip "FAIL:门内无负例判据缺口；根因链红测锁定"

# CORE-70-R10：真实专属门（2026-09-17）——三窗实弹逐意图证据。
rec CORE-70-R10 R10-HELD r10-held.log \
  "tools/run70-r10-held.sh（段A 健康多时段 480s + 段B 脏断电重启续接 480s×2 + 段C 0x201 拒写部分失败 480s）" L3 qemu-test \
  target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  evidence/70/70.15/r10-held/image.sha256 \
  "=== R10-HELD-GATE: PASS ===" \
  "ADR-0054 持有链：PLAN-HELD→分步 HELD-COMMAND（valid_from 逐步生效）→Completed；重启保留已确认前缀/未发步取消/零补发（对账只读）；部分失败混合终态" \
  held "PASS:A1-A6/B0-B5/C1-C4 全 ✓" partial-failure "FAIL:段C 0x201 拒写下 0x200 健康写入与 Applied 判据必须仍在（部分失败不吞健康资产）"

# R12：候选判定库红测（跑一次并录制）
rc=0
cargo test -p eneros-xtask evidence_chain:: --locked > "$B/checker-red-tests5.log" 2>&1 || rc=$?
if [ "$rc" -eq 0 ]; then
  echo "candidate-check red tests: PASS" >> "$B/checker-red-tests5.log"
else
  echo "candidate-check red tests: FAIL" >> "$B/checker-red-tests5.log"
  exit "$rc"
fi
echo "$rc" > "$B/checker-red-tests5.exit"
VLINE=$(grep -m1 "test result: ok" "$B/checker-red-tests5.log")
python3 "$REC" record \
  --case CORE-70-R12 --gate evidence42-candidate-check --log "$B/checker-red-tests5.log" \
  --command "cargo test -p eneros-xtask evidence_chain::（候选判定库红测）" --scope GATE \
  --profile qemu-test --artifact-name qemu-test-combined --artifact target/evidence70-vol7/eneros-combined.raw \
  --ran-artifact-sha-file evidence/70/70.7/combined.sha256 \
  --verdict-line "$VLINE" \
  --oracle "host 判定库全负例形态" \
  --positive "red-tests:PASS:evidence_chain 全绿" \
  --negative "fake-pass:FAIL:单个合法信封不再能宣称整体通过" \
  || exit $?
echo "[rec] CORE-70-R12 OK"

# R11：外部阻塞登记
python3 "$REC" blocked --case CORE-70-R11 --blocker EXT-70-HW \
  --reason "实板未到货，无 bring-up 记录" \
  --profile qemu-test --artifact target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \
  && echo "[rec] R11 BLOCKED OK"

python3 "$REC" status
