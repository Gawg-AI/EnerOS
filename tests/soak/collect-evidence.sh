#!/usr/bin/env bash
# v0.254.0（D1b）：浸泡证据采集与打包器。
#
# 依据：蓝图/14-阶段D-商用验收闸门.md D1 证据形式：
# - 串口日志 + 启动时间分布（boot-chain.log + boot-times.csv）
# - 断电注入脚本 + 恢复校验（power-cut.log + recovery-check.log）
# - 流量抓包 + 点表一致性报告（pcap/ + point-consistency.json）
# - 调度日志 + 阻断事件（schedule.log + blocked-events.json）
# - 验签/回滚/证明日志（secure-boot.log / ota-rollback.log / attestation.log）
# - 导出样本 + 符号化报告（exported/ + symbolize-report.json）
#
# 从 `target/soak/` 采集原始证据 → 统一打包为带 sha256 清单的证据包
# （evidence-bundle/），供 D1 验收与阶段 D 出口评审引用。
#
# 用法：bash tests/soak/collect-evidence.sh [--bundle-dir <dir>]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

SOAK_DIR="${ENEROS_SOAK_OUT:-target/soak}"
BUNDLE_DIR="${1:-${SOAK_DIR}/evidence-bundle}"

mkdir -p "${BUNDLE_DIR}"

# ── 1. 六类证据采集（各来源缺失时产出"缺失清单"，不静默通过）──
collect_if_present() {
    local name="$1"
    local src="$2"
    local dst="$3"
    if [ -e "${src}" ]; then
        cp "${src}" "${dst}"
        echo "collected: ${name} <- ${src}"
    else
        echo "missing: ${name} (${src})" >> "${BUNDLE_DIR}/missing-evidence.txt"
    fi
}

: > "${BUNDLE_DIR}/missing-evidence.txt"

# 1. 启动链证据：串口日志 + 启动时间分布
collect_if_present "boot-chain.log" "${SOAK_DIR}/boot-chain.log" "${BUNDLE_DIR}/boot-chain.log"
collect_if_present "boot-times.csv" "${SOAK_DIR}/boot-times.csv" "${BUNDLE_DIR}/boot-times.csv"

# 2. 断电注入 + 恢复校验
collect_if_present "power-cut.log" "${SOAK_DIR}/power-cut.log" "${BUNDLE_DIR}/power-cut.log"
collect_if_present "recovery-check.log" "${SOAK_DIR}/recovery-check.log" "${BUNDLE_DIR}/recovery-check.log"

# 3. 协议对通：抓包 + 点表一致性
if [ -d "${SOAK_DIR}/pcap" ]; then
    cp -r "${SOAK_DIR}/pcap" "${BUNDLE_DIR}/pcap"
    echo "collected: pcap/ <- ${SOAK_DIR}/pcap"
else
    echo "missing: pcap/ (${SOAK_DIR}/pcap)" >> "${BUNDLE_DIR}/missing-evidence.txt"
fi
collect_if_present "point-consistency.json" "${SOAK_DIR}/point-consistency.json" "${BUNDLE_DIR}/point-consistency.json"

# 4. 决策链：调度日志 + 阻断事件
collect_if_present "schedule.log" "${SOAK_DIR}/schedule.log" "${BUNDLE_DIR}/schedule.log"
collect_if_present "blocked-events.json" "${SOAK_DIR}/blocked-events.json" "${BUNDLE_DIR}/blocked-events.json"

# 5. 安全链
collect_if_present "secure-boot.log" "${SOAK_DIR}/secure-boot.log" "${BUNDLE_DIR}/secure-boot.log"
collect_if_present "ota-rollback.log" "${SOAK_DIR}/ota-rollback.log" "${BUNDLE_DIR}/ota-rollback.log"
collect_if_present "attestation.log" "${SOAK_DIR}/attestation.log" "${BUNDLE_DIR}/attestation.log"

# 6. 运维：导出样本 + 符号化报告
if [ -d "${SOAK_DIR}/exported" ]; then
    cp -r "${SOAK_DIR}/exported" "${BUNDLE_DIR}/exported"
    echo "collected: exported/ <- ${SOAK_DIR}/exported"
else
    echo "missing: exported/ (${SOAK_DIR}/exported)" >> "${BUNDLE_DIR}/missing-evidence.txt"
fi
collect_if_present "symbolize-report.json" "${SOAK_DIR}/symbolize-report.json" "${BUNDLE_DIR}/symbolize-report.json"

# ── 2. 证据包清单（sha256，防篡改基线）──
MANIFEST="${BUNDLE_DIR}/MANIFEST.sha256"
(
    cd "${BUNDLE_DIR}"
    find . -type f -not -name "MANIFEST.sha256" -not -name "missing-evidence.txt" | sort | while read -r f; do
        sha256sum "$f"
    done
) > "${MANIFEST}"

# ── 3. 汇总 ──
COLLECTED=$(grep -c "^collected" <<<"$(cat "${BUNDLE_DIR}/missing-evidence.txt" 2>/dev/null; echo collected:)" 2>/dev/null || true)
MISSING=$(grep -c "^missing" "${BUNDLE_DIR}/missing-evidence.txt" || true)
echo ""
echo "[evidence] bundle: ${BUNDLE_DIR}"
echo "[evidence] manifest: ${MANIFEST} ($(wc -l < "${MANIFEST}") files hashed)"
echo "[evidence] missing: ${MISSING} item(s) (see missing-evidence.txt)"

if [ "${MISSING}" -gt 0 ]; then
    echo "[evidence] WARN: 存在缺失证据——浸泡未覆盖全部六类证据（硬件/外设依赖项）"
fi
exit 0
