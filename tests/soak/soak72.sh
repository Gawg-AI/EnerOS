#!/usr/bin/env bash
# v0.253.0（D1a）：真机 72h 浸泡执行器（飞腾 D2000 / 鲲鹏 920）。
#
# 依据：蓝图/14-阶段D-商用验收闸门.md D1——真机端到端浸泡：
# - 启动链：UEFI→boot-manager→seL4→root-task→EL0 全部真实 spawn，冷启动
#   100 次零失败（--boot-cycles 可调）
# - 存储：fs/tsdb/OTA/录波真机落盘，随机断电 50 次数据可恢复
# - 协议对通：Modbus/IEC104 真实设备 72h 无失联（--protocol-hours）
# - 决策链：真实 HiGHS 日前调度每日执行 + safety Fatal 注入被阻断
# - 安全链：安全启动验签、OTA-A/B 升级+回滚、attestation 远程证明
# - 运维：指标/日志真实导出，crash-dump 可采集可符号化
#
# 蓝图/20 R1-5（fail-closed 改造）：
# - 检查器/执行器分离：执行器（真实硬件命令）写证据文件；检查器读证据
#   文件校验字段——缺失/被篡改/不符 → FAIL（exit 1）。
# - 脚本内禁止字面量 "PASS" 直接写入报告的分支（CI grep 门禁）；
#   报告状态由证据推导。
# - 无硬件（无标记）→ verdict=SKIP + 就绪检查清单，exit 0。
# - 自检模式 `bash tests/soak/soak72.sh --self-test`：注入假硬件标记 +
#   篡改证据，断言三个用例全部被判 FAIL（验收标准）。
#
# 用法：
#   bash tests/soak/soak72.sh                      # 探测→跳过或全量
#   bash tests/soak/soak72.sh --self-test          # 自检（无硬件也可运行）
#   ENEROS_SOAK_FORCE=1 bash tests/soak/soak72.sh  # 强制执行（硬件就绪时）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

OUT_DIR="${ENEROS_SOAK_OUT:-target/soak}"
mkdir -p "${OUT_DIR}"

BOOT_CYCLES="${ENEROS_SOAK_BOOT_CYCLES:-100}"
POWER_CUTS="${ENEROS_SOAK_POWER_CUTS:-50}"
PROTOCOL_HOURS="${ENEROS_SOAK_PROTOCOL_HOURS:-72}"

# ============================================================
# 检查器：读取执行器证据文件 → 校验字段 → 缺失/不符即 FAIL
# （蓝图/20 R1-5：无证据=失败，证据缺失/被篡改=失败）
# ============================================================
FAIL_COUNT=0
fail() {
    echo "[soak] FAIL: $1" >&2
    FAIL_COUNT=$((FAIL_COUNT + 1))
}

# 启动链：boot-chain.log 每行 "cycle <n>: ok boot_ms=<ms>"，行数 ≥ BOOT_CYCLES
check_boot_chain() {
    local log="${OUT_DIR}/boot-chain.log"
    if [ ! -f "${log}" ]; then
        fail "missing boot-chain evidence: ${log}"
        return
    fi
    local lines
    lines=$(wc -l < "${log}" | tr -d ' ')
    if [ "${lines}" -lt "${BOOT_CYCLES}" ]; then
        fail "boot-chain log has ${lines} lines, expected >= ${BOOT_CYCLES}"
        return
    fi
    if ! grep -qE '^cycle [0-9]+: ok boot_ms=[0-9]+$' "${log}"; then
        fail "boot-chain log has malformed lines (expected 'cycle N: ok boot_ms=<ms>')"
        return
    fi
    if grep -qE 'failed|error' "${log}"; then
        fail "boot-chain log contains failure markers"
    fi
}

# 断电恢复：power-cut.log 每行 "cut <n>: ok [checksum=<hex>]"，行数 ≥ POWER_CUTS
check_power_cut() {
    local log="${OUT_DIR}/power-cut.log"
    if [ ! -f "${log}" ]; then
        fail "missing power-cut evidence: ${log}"
        return
    fi
    local lines
    lines=$(wc -l < "${log}" | tr -d ' ')
    if [ "${lines}" -lt "${POWER_CUTS}" ]; then
        fail "power-cut log has ${lines} lines, expected >= ${POWER_CUTS}"
        return
    fi
    if ! grep -qE '^cut [0-9]+: ok( checksum=[0-9a-f]+)?$' "${log}"; then
        fail "power-cut log has malformed lines (expected 'cut N: ok [checksum=<hex>]')"
        return
    fi
    if grep -qE 'failed|error|corrupt' "${log}"; then
        fail "power-cut log contains failure markers"
    fi
}

# 协议对通：protocol-soak.log 覆盖 72h 无失联
check_protocol() {
    local log="${OUT_DIR}/protocol-soak.log"
    if [ ! -f "${log}" ]; then
        fail "missing protocol-soak evidence: ${log}"
        return
    fi
    if ! grep -qE '^ok' "${log}"; then
        fail "protocol-soak log lacks ok marker"
        return
    fi
    if grep -qE 'lost|disconnect|failed|error' "${log}"; then
        fail "protocol-soak log contains loss markers"
    fi
}

# 决策链 + 安全链 + 运维：chains-daily.log 每日断言
check_chains() {
    local log="${OUT_DIR}/chains-daily.log"
    if [ ! -f "${log}" ]; then
        fail "missing daily chains evidence: ${log}"
        return
    fi
    if grep -qE 'failed|error' "${log}"; then
        fail "daily chains log contains failure markers"
    fi
}

run_checks() {
    FAIL_COUNT=0
    check_boot_chain
    check_power_cut
    check_protocol
    check_chains
}

# ============================================================
# 执行器：真实硬件命令接入点（kunpeng_boot.sh / phytium_boot.sh 扩展）。
# 接入前不产生任何证据 → 检查器按"无证据=失败"判 FAIL（fail-closed）。
# 接入后每个 cycle/cut 写一行 "…: ok …" 到对应日志。
# ============================================================
run_executors() {
    # 42 零硬件验收：优先使用 QEMU software adapter（cargo xtask qa soak-run）
    # 作为真实执行器；未配置任何 adapter 时明确 BLOCKED，禁止空函数占位。
    if [ -n "${ENEROS_SOAK_QEMU_IMAGE:-}" ]; then
        local cycles="${ENEROS_SOAK_QEMU_CYCLES:-10}"
        local vars_args=()
        if [ -n "${ENEROS_SOAK_QEMU_VARS:-}" ]; then
            vars_args=(--vars-template "${ENEROS_SOAK_QEMU_VARS}")
        fi
        cargo xtask qa soak-run --cycles "${cycles}" --image "${ENEROS_SOAK_QEMU_IMAGE}"             "${vars_args[@]}" --min-duration-days 0 --out "${OUT_DIR}/soak-campaign.json"
        return
    fi

    # 真实硬件接入点（43 L4）：
    # 1. 加载 UEFI 应用，断言串口序列 [kernel] booting + [eneros-root-task]
    #    alive + EL0 banner，逐 cycle 写 boot-chain.log（含耗时 boot_ms）
    # 2. TSDB 写样本 → 随机时刻断电（SSH 电源控制器）→ 恢复 → 校验和一致，
    #    逐 cut 写 power-cut.log
    # 3. Modbus/IEC104 对通轮询 72h，写 protocol-soak.log
    # 4. 每日执行决策/安全/运维断言，写 chains-daily.log
    cat > "${OUT_DIR}/soak-empty-executor-evidence.json" <<'EOF'
{
  "schema": "eneros.soak-executor.v1",
  "verdict": "BLOCKED",
  "non_release": true,
  "reason": "no QEMU image (ENEROS_SOAK_QEMU_IMAGE) and no real hardware executor configured"
}
EOF
    echo "[soak] BLOCKED: no software/hardware executor configured" >&2
    exit 1
}

# ============================================================
# 自检模式：注入假硬件标记 + 篡改证据，断言三个用例全部 FAIL
# （蓝图/20 R1-5 验收标准：--self-test 三个篡改用例全部被判 FAIL）
# ============================================================
self_test() {
    local ok=1
    local orig_out="${OUT_DIR}"
    local i

    # 用例 1：假硬件标记 + 零证据 → 每项检查 FAIL
    OUT_DIR="$(mktemp -d)"
    run_checks
    if [ "${FAIL_COUNT}" -eq 0 ]; then
        echo "[self-test] case 1 (fake hardware, zero evidence) was NOT rejected" >&2
        ok=0
    fi

    # 用例 2：boot 证据被篡改（行数不足 + 伪造行），其余证据齐全
    #         → 仅 boot 检查 FAIL（证明 FAIL 由篡改触发，非连带）
    OUT_DIR="$(mktemp -d)"
    printf 'cycle 1: ok boot_ms=5\n' > "${OUT_DIR}/boot-chain.log"
    for i in $(seq 1 "${POWER_CUTS}"); do
        printf 'cut %s: ok\n' "${i}" >> "${OUT_DIR}/power-cut.log"
    done
    printf 'ok\n' > "${OUT_DIR}/protocol-soak.log"
    printf 'ok\n' > "${OUT_DIR}/chains-daily.log"
    run_checks
    if [ "${FAIL_COUNT}" -ne 1 ]; then
        echo "[self-test] case 2 (tampered boot evidence) FAIL_COUNT=${FAIL_COUNT}, expected 1" >&2
        ok=0
    fi

    # 用例 3：power-cut 证据被篡改（行数足够但格式不符），其余证据齐全
    #         → 仅 power-cut 检查 FAIL
    OUT_DIR="$(mktemp -d)"
    for i in $(seq 1 "${BOOT_CYCLES}"); do
        printf 'cycle %s: ok boot_ms=%s\n' "${i}" "$((i % 100))" >> "${OUT_DIR}/boot-chain.log"
    done
    for i in $(seq 1 "${POWER_CUTS}"); do
        printf 'cut %s: ok (placeholder, hardware path)\n' "${i}" >> "${OUT_DIR}/power-cut.log"
    done
    printf 'ok\n' > "${OUT_DIR}/protocol-soak.log"
    printf 'ok\n' > "${OUT_DIR}/chains-daily.log"
    run_checks
    if [ "${FAIL_COUNT}" -ne 1 ]; then
        echo "[self-test] case 3 (tampered power-cut evidence) FAIL_COUNT=${FAIL_COUNT}, expected 1" >&2
        ok=0
    fi

    # 用例 4（70.14）：运行记录判据——"参数声称 72h"但实测窗口只有 2h
    #         必须被判负；同记录的诚实版本（真实 72h）必须通过。
    OUT_DIR="$(mktemp -d)"
    printf 'boot ok\n' > "${OUT_DIR}/serial.log"
    write_run_manifest() {
        local hours="$1" out="$2"
        python3 - "${hours}" "${out}" <<'PY_RUN'
import json, sys
hours = int(sys.argv[1]); out = sys.argv[2]
def ts(mins):
    # 2026-09-01T00:00:00Z + mins（固定基准，测试夹具）
    import datetime
    base = datetime.datetime(2026, 9, 1, tzinfo=datetime.timezone.utc)
    return (base + datetime.timedelta(minutes=mins)).strftime("%Y-%m-%dT%H:%M:%SZ")
hb = [ts(m) for m in range(0, hours * 60 + 1, 10)]
metrics = [{"at": ts(m), "heap_bytes": 1000000 + (m % 50),
            "dma_bytes": 250000, "handles": 40,
            "flush_latency_ms_max": 12, "control_period_ms_max": 100}
           for m in range(0, hours * 60 + 1, 60)]
body = {
  "schema": "eneros.soak-run.v1",
  "run_id": "self-test-run",
  "scenario": "steady",
  "scope": "hw-soak",
  "artifacts": {"root_task": "a"*64, "boot_chain": "b"*64},
  "device": {"sku_id": "self-test-sku", "board_revision": "rev-A",
             "serial": "SN-SELFTEST", "firmware": "uefi-1"},
  "rig": {"rig_id": "rig-self-test", "instruments": ["power-meter"],
          "power_channel": "psu-1"},
  "started_at": ts(0), "ended_at": ts(hours * 60),
  "heartbeats": hb,
  "protocol_coverage": [
     {"protocol": "modbus-tcp", "active_from": ts(0), "active_to": ts(hours*60), "samples": 1000},
     {"protocol": "iec104", "active_from": ts(0), "active_to": ts(hours*60), "samples": 500}],
  "metrics": metrics,
  "judge_version": "self-test",
  "raw_files": ["serial.log"],
  "interruptions": []
}
json.dump(body, open(out, "w"))
PY_RUN
    }
    # 4a：诚实 72h → 判据通过
    write_run_manifest 72 "${OUT_DIR}/soak-run.json"
    if ! cargo xtask soak run-check "${OUT_DIR}/soak-run.json" --min-hours 72 \
            --base-dir "${OUT_DIR}" --out "${OUT_DIR}/verdict-ok.json" > /dev/null 2>&1; then
        echo "[self-test] case 4a (honest 72h run) was rejected" >&2
        ok=0
    fi
    # 4b：实测窗口 2h（参数/声明声称 72h 的形态）→ 判负
    write_run_manifest 2 "${OUT_DIR}/soak-run.json"
    if cargo xtask soak run-check "${OUT_DIR}/soak-run.json" --min-hours 72 \
            --base-dir "${OUT_DIR}" > /dev/null 2>&1; then
        echo "[self-test] case 4b (claimed 72h, measured 2h) was accepted" >&2
        ok=0
    fi
    OUT_DIR="${orig_out}"

    if [ "${ok}" -eq 1 ]; then
        echo "[self-test] ALL 4 TAMPER CASES REJECTED (ok)"
        exit 0
    fi
    echo "[self-test] FAILED: at least one tamper case was accepted" >&2
    exit 1
}

# ── 自检入口 ──
if [ "${1:-}" = "--self-test" ]; then
    self_test
fi

# ── 1. 硬件探测（双国产平台）──
HARDWARE=""
if [ -e /sys/class/phytium-d2000-marker ] || [ -n "${ENEROS_SOAK_HW:-}" ]; then
    HARDWARE="phytium-d2000"
elif [ -e /sys/class/kunpeng-920-marker ]; then
    HARDWARE="kunpeng-920"
fi

if [ -z "${HARDWARE}" ] && [ -z "${ENEROS_SOAK_FORCE:-}" ]; then
    echo "[soak] skip: no Phytium D2000 / Kunpeng 920 hardware detected"
    cat > "${OUT_DIR}/soak-skip-evidence.json" <<'EOF'
{
  "schema": "eneros.soak-skip-evidence.v1",
  "verdict": "PENDING",
  "non_release": true,
  "reason": "no hardware detected; no 72h soak execution",
  "hardware_required": ["phytium-d2000", "kunpeng-920"],
  "readiness_checks": {
    "boot_chain": "UEFI -> boot-manager -> seL4 -> root-task -> EL0 spawn",
    "boot_cycles": 100,
    "power_cuts": 50,
    "protocol_hours": 72,
    "decision_chain": "HiGHS day-ahead daily + safety Fatal injection",
    "security_chain": "secure boot verify + OTA A/B rollback + attestation",
    "ops": "metrics/log export + crash-dump symbolize"
  }
}
EOF
    echo "[soak] readiness checklist written to ${OUT_DIR}/soak-skip-evidence.json"
    echo "[soak] 真机浸泡留待硬件就绪（蓝图 36 WP-36.10 #3：无执行器/无硬件 = PENDING，非 PASS，退出非零）"
    # v0.254.0（D1b）：跳过路径也产证据包（跳过证据 + 缺失清单），
    # 使 CI 门禁可验证"证据采集器本身工作正常"。
    bash "${SCRIPT_DIR}/collect-evidence.sh" --allow-missing > /dev/null 2>&1 || true
    exit 1
fi

echo "[soak] hardware=${HARDWARE} boot_cycles=${BOOT_CYCLES} power_cuts=${POWER_CUTS} protocol_hours=${PROTOCOL_HOURS}"

# ── 2. 执行器（真实硬件命令）→ 检查器（读证据，fail-closed）──
# 蓝图 36 WP-36.10 #3：空执行器（仅占位注释）只能 PENDING 并非零——
# 不允许"预置少量日志"冒充 72h 浸泡。
if awk '/^run_executors\(\) \{/,/^\}/' "${BASH_SOURCE[0]}" | grep -qE '^    :$'; then
    cat > "${OUT_DIR}/soak-empty-executor-evidence.json" <<'EOF'
{
  "schema": "eneros.soak-executor.v1",
  "verdict": "PENDING",
  "non_release": true,
  "reason": "run_executors is an empty placeholder; no real 72h execution"
}
EOF
    echo "[soak] PENDING: run_executors is empty (no real executor) - not PASS" >&2
    exit 1
fi
run_executors
run_checks

# ── 2b. 运行记录判据（70.14 WP-70.14.1/4：真实起止/心跳/覆盖/样本序列）──
# 旧实现的缺口（蓝图 §9 D6-13 复核属实）：duration_days 由参数推导、
# artifact 固定读 qemu-prod 路径、memory_leak 字面零写入。现在这些都必须
# 来自 `soak-run.json`（执行器写原始事实），并由 xtask 判据核对。
RUN_MANIFEST="${OUT_DIR}/soak-run.json"
RUN_CHECK_ARGS=(run-check "${RUN_MANIFEST}" --min-hours "$((PROTOCOL_HOURS))")
MEASURED_HOURS=""
MEASURED_ARTIFACT=""
if [ ! -f "${RUN_MANIFEST}" ]; then
    fail "missing run manifest: ${RUN_MANIFEST}（真实起止/心跳/协议覆盖/样本序列的唯一来源）"
else
    if cargo xtask soak "${RUN_CHECK_ARGS[@]}" --base-dir "${OUT_DIR}" \
            --out "${OUT_DIR}/soak-run-verdict.json"; then
        MEASURED_HOURS=$(python3 -c "import json,sys;print(json.load(open('${OUT_DIR}/soak-run-verdict.json'))['measured_hours'])" 2>/dev/null || echo "")
        MEASURED_ARTIFACT=$(python3 -c "import json,sys;m=json.load(open('${RUN_MANIFEST}'));print(next(iter(m['artifacts'].values()),''))" 2>/dev/null || echo "")
    else
        fail "soak run-check rejected ${RUN_MANIFEST}"
    fi
fi

# ── 3. 产出 QA-018 证据（verdict 由检查结果推导）──
# duration_days 只作**对照**记录；正式判据是 run-check 实测小时数（无实测
# 即无结论，不再由参数推算）。
DURATION_DAYS=""
if [ -n "${MEASURED_HOURS}" ]; then
    DURATION_DAYS=$(( MEASURED_HOURS / 24 ))
fi

if [ "${FAIL_COUNT}" -gt 0 ]; then
    V_VERDICT="FAIL"
    echo "[soak] VERDICT: ${V_VERDICT} (${FAIL_COUNT} check(s) failed)"
    cat > "${OUT_DIR}/soak-campaign.json" <<EOF
{
  "schema": "eneros.soak-campaign.v1",
  "campaign_id": "real-hardware-${HARDWARE}-${BOOT_CYCLES}x",
  "verdict": "${V_VERDICT}",
  "fail_checks": ${FAIL_COUNT},
  "measured_hours": "${MEASURED_HOURS}",
  "duration_days_measured": "${DURATION_DAYS}",
  "run_check": "soak-run-verdict.json"
}
EOF
    exit 1
fi

V_VERDICT="PASS"
echo "[soak] VERDICT: ${V_VERDICT} — all checks derived from evidence files"
# 候选绑定（70.14）：产物摘要取自**本次运行记录**绑定的产物——不再固定读
# qemu-prod 路径（真机 SKU 的产物摘要不在那里），也不再允许字面零摘要。
ARTIFACT_HASH="${MEASURED_ARTIFACT}"
if [ -z "${ARTIFACT_HASH}" ]; then
    V_VERDICT="FAIL"
    echo "[soak] FAIL: artifact_sha256 empty (no candidate artifact bound)" >&2
    cat > "${OUT_DIR}/soak-campaign.json" <<FAILED_EOF
{
  "schema": "eneros.soak-campaign.v1",
  "campaign_id": "real-hardware-${HARDWARE}-${BOOT_CYCLES}x",
  "artifact_sha256": "",
  "verdict": "${V_VERDICT}",
  "reason": "no candidate artifact bound to soak evidence",
  "duration_days": ${DURATION_DAYS}
}
FAILED_EOF
    exit 1
fi
cat > "${OUT_DIR}/soak-campaign.json" <<CAMPAIGN_EOF
{
  "schema": "eneros.soak-campaign.v1",
  "campaign_id": "real-hardware-${HARDWARE}-${BOOT_CYCLES}x",
  "artifact_sha256": "${ARTIFACT_HASH}",
  "verdict": "${V_VERDICT}",
  "measured_hours": ${MEASURED_HOURS},
  "duration_days_measured": ${DURATION_DAYS},
  "min_duration_hours": ${PROTOCOL_HOURS},
  "run_check": "soak-run-verdict.json",
  "resource_metrics": "derived-from-run-manifest (见 soak-run-verdict.json: heap_peak/heap_slope/dma_peak/handles_peak)"
}
CAMPAIGN_EOF
echo "[soak] campaign evidence: ${OUT_DIR}/soak-campaign.json"
echo "[soak] DONE"
# v0.254.0（D1b）：打包六类证据（含 sha256 清单）。
# 70.14：缺必需材料 → 非零退出（旧实现 `|| true` 吞掉失败——"打包完成"被
# 当成"证据齐全"）。
if ! bash "${SCRIPT_DIR}/collect-evidence.sh"; then
    echo "[soak] FAIL: evidence bundle incomplete (see missing-evidence.txt)" >&2
    exit 1
fi
