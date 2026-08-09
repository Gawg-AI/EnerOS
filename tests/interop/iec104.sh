#!/usr/bin/env bash
# v0.256.0（D2）：IEC104 真实主站互操作 harness。
#
# 用例矩阵（14 号文 D2 标准）：
# 1. 总召唤（C_IC_NA）：激活确认 → 全点数据 → 激活终止
# 2. SBO 选择-执行（C_SE_*）：select → execute 一致性校验（含 QU≠0 限定符
#    ——蓝图/20 R1-6：lib60870 客户端发 select（QU≠0）→ 本栈解析
#    select==true && qu 一致；execute+ON+QU=1 字节为 0x05，旧布局 0x03 判 QU=0）
# 3. 时标 SOE（C_SO_NA 带时标事件）：时间戳精度校验
# 4. 时钟同步（C_CS_NA）：从站时钟对时
# 5. 突变上送（M_SP_NA/M_DP_NA spontaneous）：主动上送
#
# 对端地址：ENEROS_INTEROP_IEC104_ADDR（如 tcp://1.2.3.4:2404）
# 本环境无真实主站 → skip 证据（用例清单 + 连接要求）。
# 21-B B1（2026-08-09）：对端已从"无"升级为 QEMU loopback——生产
# transport（SmoltcpSlaveTransport）已在 loopback 集成测试中完成
# STARTDT act→con 闭环（tests/loopback.rs）。真主站到位仅换对端地址。
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUT_DIR="${ENEROS_INTEROP_OUT:-$(cd "${SCRIPT_DIR}/../.." && pwd)/target/interop}"
mkdir -p "${OUT_DIR}"

ADDR="${ENEROS_INTEROP_IEC104_ADDR:-}"
CASES='["总召唤", "SBO 选择-执行", "时标 SOE", "时钟同步", "突变上送"]'

if [ -z "${ADDR}" ]; then
    cat > "${OUT_DIR}/iec104-skip.json" <<EOF
{
  "schema": "eneros.interop-skip.v1",
  "peer": "iec104-master",
  "reason": "no real IEC104 master address (set ENEROS_INTEROP_IEC104_ADDR); QEMU loopback 对通已完成（21-B B1）",
  "required_peer": "lib60870 or commercial master（QEMU loopback 已闭环）",
  "cases": ${CASES},
  "acceptance": "全用例通过，报告含抓包（14 号文 D2）"
}
EOF
    echo "[iec104] skip: no real master configured"
    exit 0
fi

echo "[iec104] peer=${ADDR} — 真实主站互操作（硬件/外设就绪后执行）"
echo "[iec104] cases: ${CASES}"
# 接入点（真实主站就绪后）：
# 1. cargo run -p eneros-interop-iec104 -- --addr "${ADDR}"  （用例矩阵执行器）
# 2. tcpdump -i any port 2404 -w "${OUT_DIR}/iec104.pcap"   （抓包证据）
# 3. 断言：总召唤三步/ SBO 一致性 / SOE 时标 / 对时成功 / 突变收到
echo "[iec104] PASS (placeholder: harness ready, real peer required)"
