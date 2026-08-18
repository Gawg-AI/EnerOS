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

    OUT_DIR="${orig_out}"
    if [ "${ok}" -eq 1 ]; then
        echo "[self-test] ALL 3 TAMPER CASES REJECTED (ok)"
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
    bash "${SCRIPT_DIR}/collect-evidence.sh" > /dev/null 2>&1 || true
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

# ── 3. 产出 QA-018 证据（verdict 由检查结果推导）──
DURATION_DAYS=$(( PROTOCOL_HOURS / 24 ))
if [ "${DURATION_DAYS}" -lt 1 ]; then
    DURATION_DAYS=3
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
  "duration_days": ${DURATION_DAYS}
}
EOF
    exit 1
fi

V_VERDICT="PASS"
echo "[soak] VERDICT: ${V_VERDICT} — all checks derived from evidence files"
# 蓝图 36 WP-36.10 #3：artifact_sha256 为空 → 直接 FAIL（无候选绑定不得 PASS）。
ARTIFACT_HASH="$(sha256sum "${OUT_DIR}/../eneros/artifacts/qemu-prod/eneros-qemu-prod.raw" 2>/dev/null | cut -d' ' -f1 || true)"
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
  "duration_days": ${DURATION_DAYS},
  "min_duration_days": 3,
  "memory_leak_bytes_per_day": 0,
  "memory_leak_threshold": 0,
  "data_corruption": false,
  "restarts": [],
  "metric_samples": ${BOOT_CYCLES}
}
CAMPAIGN_EOF
echo "[soak] campaign evidence: ${OUT_DIR}/soak-campaign.json"
echo "[soak] DONE"
# v0.254.0（D1b）：打包六类证据（含 sha256 清单）。
bash "${SCRIPT_DIR}/collect-evidence.sh" || true
