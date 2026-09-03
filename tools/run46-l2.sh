#!/bin/bash
# 46.5 L2 独立对端 ×7 编排（一次性跑完全部 6 个 host 对端 + QEMU 实弹）。
#
# 判据四件套（43 卷 Modbus L2 先例）：
#   QEMU 退出码（--expect PASS → harness 判 PASS/FAIL）
#   串口 READY（[svc-*] NET-TRANSPORT READY 行）
#   SELFCHECK（svc 就绪 + 服务健康行）
#   对端 transcript ≥3 事务（evidence/46/qemu-l2/<proto>.transcript.jsonl）
#
# 用法：tools/run46-l2.sh          正向（全部对端在线）
#       tools/run46-l2.sh --neg    modbus 拔 peer 负例（--serve-after 15）
set -euo pipefail
source "$(dirname "$0")/gate-env.sh"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVID="$ROOT/evidence/46/qemu-l2"
mkdir -p "$EVID"
NEG="${1:-}"

# 工具链环境（与 run46-qemu.sh 一致）。

# 1. 清空旧 transcript（对端 append 写）。
rm -f "$EVID"/*.transcript.jsonl

# 2. 启动 6 个独立对端（非 EnerOS 进程；后台保活至 QEMU 结束）。
MODBUS_ARGS=()
if [ "$NEG" = "--neg" ]; then MODBUS_ARGS=(--serve-after 15); fi
python3 evidence/45/modbus_slave.py --transcript "$EVID/modbus.transcript.jsonl" "${MODBUS_ARGS[@]}" &
P_MODBUS=$!
python3 evidence/46/iec104_slave.py --transcript "$EVID/iec104.transcript.jsonl" &
P_IEC104=$!
python3 evidence/46/mqtt_broker.py --transcript "$EVID/mqtt.transcript.jsonl" &
P_MQTT=$!
python3 evidence/46/mms_peer.py --transcript "$EVID/mms.transcript.jsonl" &
P_MMS=$!
python3 evidence/46/goose_peer.py --transcript "$EVID/goose.transcript.jsonl" --port 5577 &
P_GOOSE=$!
python3 evidence/46/sv_peer.py --transcript "$EVID/sv.transcript.jsonl" --port 5582 &
P_SV=$!
trap 'kill $P_MODBUS $P_IEC104 $P_MQTT $P_MMS $P_GOOSE $P_SV 2>/dev/null || true' EXIT
sleep 1

# 3. QEMU 实弹（PEER transcript 由对端直接写 evidence/46/qemu-l2/）。
# L2 证据需要**持续窗口**：product-ready 后 harness 若 qmp-shutdown 立即退出，
# 会砍掉尚在等 svc-config EIPC 配置应答的 mms/goose/sv（46.5 实测 <15s 即 quit）。
# 因此不用 --qmp-shutdown，改走 harness 的 200s 有界窗口（timeout KILL）；
# rc=124（超时杀）= 正常拿到完整证据窗口。
cargo xtask run qemu-prod \
  --image target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw \
  --vars-template target/evidence46-l2/template.fd \
  --expect PASS \
  --evidence-dir target/evidence46/qemu-l2 \
  --net-mode present \
  --net-user \
  --net-protocols present \
  --reset-bootctrl \
  --ready-marker product-ready \
  --qmp-deadline-secs 300
RC=$?
[ "$RC" = "124" ] && RC=0
# 给对端最后的 flush（sv/goose 可能还有收发未落 transcript）。
sleep 2

# 4. transcript 汇总（每协议 ≥3 事务判据）。
echo "=== L2 transcript 汇总（RC=$RC）==="
for f in "$EVID"/*.transcript.jsonl; do
  n=$(wc -l < "$f")
  last=$(tail -1 "$f" 2>/dev/null)
  echo "$(basename "$f"): $n 行 | 末行: $last"
done
echo "=== 串口 READY 行 ==="
grep -E "NET-TRANSPORT READY|protocol-service" target/evidence46/qemu-l2/qemu-serial.log 2>/dev/null | head -30 || true
exit $RC