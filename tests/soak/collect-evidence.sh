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
# 70.14（WP-70.14.1）判据变更：
# - 缺必需材料 = **非零退出**（旧实现无论缺多少都 exit 0，无法与完整包区分）；
# - `--allow-missing` 显式进入"缺件报告"模式（non_release），产物标注
#   verdict=INCOMPLETE 且不得作为技术资格证据（无硬件环境的开发回归用）。
#
# 用法：bash tests/soak/collect-evidence.sh [--bundle-dir <dir>] [--allow-missing]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

SOAK_DIR="${ENEROS_SOAK_OUT:-target/soak}"
ALLOW_MISSING=0
BUNDLE_DIR=""
while [ $# -gt 0 ]; do
    case "$1" in
        --bundle-dir) BUNDLE_DIR="${2:-}"; shift 2 ;;
        --allow-missing) ALLOW_MISSING=1; shift ;;
        -h|--help)
            echo "usage: collect-evidence.sh [--bundle-dir <dir>] [--allow-missing]"
            exit 0 ;;
        *) echo "[evidence] unknown arg: $1" >&2; exit 2 ;;
    esac
done
if [ -z "${BUNDLE_DIR}" ]; then
    BUNDLE_DIR="${SOAK_DIR}/evidence-bundle"
fi

mkdir -p "${BUNDLE_DIR}"

# ── 1. 六类证据采集（各来源缺失时产出"缺失清单"，不静默通过）──
collect_if_present() {
    local name="$1"
    local src="$2"
    local dst="$3"
    # 70.14（S7 审计 P3）：零字节"证据文件"不算证据——旧实现只看存在性，
    # 11 个空文件也能自报 COMPLETE。
    if [ -s "${src}" ]; then
        cp "${src}" "${dst}"
        echo "collected: ${name} <- ${src}"
    elif [ -e "${src}" ]; then
        echo "empty: ${name} (${src})" >> "${BUNDLE_DIR}/missing-evidence.txt"
    else
        echo "missing: ${name} (${src})" >> "${BUNDLE_DIR}/missing-evidence.txt"
    fi
}

# 目录类证据（pcap/exported）：必须存在且含至少一个非空文件。
#
# 70.14 复查修复：旧判据 `find … -size +0c | head -1 | grep -q .` 在本脚本的
# `set -o pipefail` 下**把有内容的大目录判成空**——`grep -q` 命中首个匹配即
# 退出，`head`/`find` 依次收到 SIGPIPE，管道状态 141 ⇒ 条件为假 ⇒ 归 `empty:`
# ⇒ 严格模式 exit 1 / 宽松模式自标 non_release。实测（3000 个非空文件的目录）：
# `rc=141 JUDGED_EMPTY`——证据包越完整越容易被判缺件，fail-closed 方向反了。
# 改为 `find -print -quit`（找到第一个即停，无管道、无 SIGPIPE）。
collect_dir_if_present() {
    local name="$1"
    local src="$2"
    local dst="$3"
    local first_nonempty=""
    if [ -d "${src}" ]; then
        first_nonempty="$(find "${src}" -type f -size +0c -print -quit 2>/dev/null || true)"
    fi
    if [ -n "${first_nonempty}" ]; then
        cp -r "${src}" "${dst}"
        echo "collected: ${name} <- ${src}"
    elif [ -d "${src}" ]; then
        echo "empty: ${name}/ (${src})" >> "${BUNDLE_DIR}/missing-evidence.txt"
    else
        echo "missing: ${name}/ (${src})" >> "${BUNDLE_DIR}/missing-evidence.txt"
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
collect_dir_if_present "pcap" "${SOAK_DIR}/pcap" "${BUNDLE_DIR}/pcap"
collect_if_present "point-consistency.json" "${SOAK_DIR}/point-consistency.json" "${BUNDLE_DIR}/point-consistency.json"

# 4. 决策链：调度日志 + 阻断事件
collect_if_present "schedule.log" "${SOAK_DIR}/schedule.log" "${BUNDLE_DIR}/schedule.log"
collect_if_present "blocked-events.json" "${SOAK_DIR}/blocked-events.json" "${BUNDLE_DIR}/blocked-events.json"

# 5. 安全链
collect_if_present "secure-boot.log" "${SOAK_DIR}/secure-boot.log" "${BUNDLE_DIR}/secure-boot.log"
collect_if_present "ota-rollback.log" "${SOAK_DIR}/ota-rollback.log" "${BUNDLE_DIR}/ota-rollback.log"
collect_if_present "attestation.log" "${SOAK_DIR}/attestation.log" "${BUNDLE_DIR}/attestation.log"

# 6. 运维：导出样本 + 符号化报告
collect_dir_if_present "exported" "${SOAK_DIR}/exported" "${BUNDLE_DIR}/exported"
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
MISSING=$(grep -cE "^(missing|empty):" "${BUNDLE_DIR}/missing-evidence.txt" || true)
echo ""
echo "[evidence] bundle: ${BUNDLE_DIR}"
echo "[evidence] manifest: ${MANIFEST} ($(wc -l < "${MANIFEST}") files hashed)"
echo "[evidence] missing: ${MISSING} item(s) (see missing-evidence.txt)"

if [ "${MISSING}" -gt 0 ]; then
    echo "[evidence] FAIL: 存在缺失证据——浸泡未覆盖全部必需证据类（硬件/外设依赖项）" >&2
    # 70.14（WP-70.14.1）：「打包完成」与「证据齐全」是两件事。旧实现无论
    # 缺多少材料都 exit 0（蓝图 §9 D6-14 复核属实），下游无法区分"完整包"
    # 与"只有清单的包"。现在：
    # - 默认（无 --allow-missing）：缺必需材料 → 非零退出（fail-closed；
    #   soak72.sh 与 CI 都按失败处理）；
    # - `--allow-missing`：显式登记为"缺件报告"（用于无硬件环境的开发回归），
    #   该模式下的产物**不得**作为技术资格证据。
    if [ "${ALLOW_MISSING}" -eq 1 ]; then
        echo "[evidence] allow-missing: 缺件报告模式（non_release）——不得作为技术资格证据" >&2
        printf '{\n  "schema": "eneros.evidence-bundle-verdict.v1",\n  "verdict": "INCOMPLETE",\n  "non_release": true,\n  "missing": %s\n}\n' "${MISSING}" \
            > "${BUNDLE_DIR}/bundle-verdict.json"
        exit 0
    fi
    printf '{\n  "schema": "eneros.evidence-bundle-verdict.v1",\n  "verdict": "FAIL",\n  "missing": %s\n}\n' "${MISSING}" \
        > "${BUNDLE_DIR}/bundle-verdict.json"
    exit 1
fi

printf '{\n  "schema": "eneros.evidence-bundle-verdict.v1",\n  "verdict": "COMPLETE",\n  "missing": 0\n}\n' \
    > "${BUNDLE_DIR}/bundle-verdict.json"
echo "[evidence] OK: 全部必需材料齐备"
exit 0
