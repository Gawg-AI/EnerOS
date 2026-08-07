#!/usr/bin/env bash
# v0.256.0（D2）：第三方互操作测试框架（RC-2）。
#
# 依据：蓝图/14-阶段D-商用验收闸门.md D2——四类对端互操作：
# - 真实 IEC104 主站（lib60870 或商用主站）：总召唤/SBO/时标 SOE/时钟同步/突变上送
# - 真实 Modbus 设备（≥2 款主流 PLC/电表）：全功能码、异常码、边界地址、长时轮询
# - 真实 MMS 客户端（libiec61850）：关联/读写/报告/GOOSE 订阅
# - MQTT broker（≥2 款）：TLS+QoS0/1/2+断链重连
#
# 框架结构：每类对端一个 harness（tests/interop/<peer>.sh），统一入口
# `interop.sh <peer>`：
# - 对端可用（环境变量注入地址）→ 执行完整用例矩阵，产出互操作矩阵报告
# - 对端不可用 → 产出 skip 证据（用例清单 + 连接要求），exit 0
#
# 用法：
#   bash tests/interop/interop.sh iec104     # 或 modbus / mms / mqtt / all
#   ENEROS_INTEROP_IEC104_ADDR=tcp://1.2.3.4:2404 bash tests/interop/interop.sh iec104
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

OUT_DIR="${ENEROS_INTEROP_OUT:-target/interop}"
mkdir -p "${OUT_DIR}"

run_peer() {
    local peer="$1"
    local script="${SCRIPT_DIR}/${peer}.sh"
    if [ ! -f "${script}" ]; then
        echo "interop: unknown peer '${peer}' (expected: iec104|modbus|mms|mqtt)" >&2
        return 2
    fi
    bash "${script}"
}

case "${1:-all}" in
    all)
        for p in iec104 modbus mms mqtt; do
            run_peer "$p"
        done
        ;;
    iec104|modbus|mms|mqtt)
        run_peer "$1"
        ;;
    *)
        echo "usage: interop.sh <iec104|modbus|mms|mqtt|all>" >&2
        exit 2
        ;;
esac

echo "[interop] framework done — reports in ${OUT_DIR}/"
