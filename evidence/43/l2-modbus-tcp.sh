#!/usr/bin/env bash
# 43 L2 协议互操作：Modbus-TCP 产品端点 ↔ 独立 raw 从站（QEMU）。
# - 独立从站：evidence/43/modbus_l2_slave.py（socket+struct，零 EnerOS 代码）；
# - 产品端点：正式介质 svc-modbus-tcp（root 移交 virtio-net 槽 8 →
#   VirtioNetDriver → smoltcp → ModbusTcpMaster 读保持寄存器）；
# - 网络：QEMU user-mode（slirp）——guest 10.0.2.2:5502 → host 127.0.0.1:5502。
# 成功判据（全部满足才 PASS）：
#   1. QEMU 受控退出（product-ready, qmp exit 0）——timeout = NO_RESULT；
#   2. 串口含 `NET-TRANSPORT READY capability=net-modbus-tcp`；
#   3. 串口含 `MODBUS-L2-SELFCHECK ok values=[...]`（读回真实值）；
#   4. 独立对端 transcript 含 ≥3 条 REQUEST（0x1111/0x2222/0x3333）且
#      handled=ok（外部可观察副作用）。
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

IMAGE=${IMAGE:-target/eneros/artifacts/qemu-prod/eneros-qemu-prod-43.raw}
VARS=target/eneros/artifacts/qemu-prod/AAVMF_VARS.fd
EVDIR=${EVDIR:-target/evidence43/l2-modbus-tcp}
TRANSCRIPT=$EVDIR/peer-transcript.jsonl
SLAVE_PID=

rm -rf "$EVDIR"; mkdir -p "$EVDIR"

echo "== start independent raw Modbus-TCP slave (host 127.0.0.1:5502) =="
python3 evidence/43/modbus_l2_slave.py --port 5502 --transcript "$TRANSCRIPT" \
  >"$EVDIR/slave.log" 2>&1 &
SLAVE_PID=$!
sleep 1
grep -q SLAVE-READY "$EVDIR/slave.log" || { echo "slave failed to start" >&2; cat "$EVDIR/slave.log" >&2; exit 2; }

trap 'kill $SLAVE_PID 2>/dev/null || true' EXIT

echo "== run QEMU formal media with virtio-net slot8 (user-mode) =="
timeout 560 cargo xtask run qemu-prod \
  --image "$IMAGE" \
  --vars-template "$VARS" \
  --evidence-dir "$EVDIR" \
  --expect PASS \
  --qmp-shutdown \
  --reset-bootctrl \
  --net-mode absent \
  --net-user \
  --ready-marker product-ready \
  --qmp-deadline-secs 420

sleep 1
kill $SLAVE_PID 2>/dev/null || true
trap - EXIT

echo "== oracle: serial evidence =="
LOG=$EVDIR/qemu-serial.log
test -s "$LOG" || { echo "serial log missing" >&2; exit 1; }
grep -Fq "NET-TRANSPORT READY capability=net-modbus-tcp" "$LOG" || { echo "MISS net-transport-ready" >&2; exit 1; }
grep -Fq "MODBUS-L2-SELFCHECK ok values=" "$LOG" || { echo "MISS modbus-l2-selfcheck-ok" >&2; exit 1; }
VALUES=$(grep -oE "MODBUS-L2-SELFCHECK ok values=\[[^]]*\]" "$LOG" | tail -1)
echo "  $VALUES"

echo "== oracle: independent peer transcript =="
test -s "$TRANSCRIPT" || { echo "transcript empty" >&2; exit 1; }
REQS=$(grep -c '"event": "REQUEST"' "$TRANSCRIPT" || true)
OKREQS=$(grep -c '"handled": "ok"' "$TRANSCRIPT" || true)
CONNS=$(grep -c '"event": "CONNECT"' "$TRANSCRIPT" || true)
echo "  connections=$CONNS requests=$REQS ok=$OKREQS"
[ "$CONNS" -ge 1 ] || { echo "no peer connection" >&2; exit 1; }
[ "$REQS" -ge 3 ] || { echo "insufficient transactions ($REQS<3)" >&2; exit 1; }
[ "$OKREQS" -ge 3 ] || { echo "not all transactions handled ok" >&2; exit 1; }

echo "L2-MODBUS-TCP VERIFIED values=$VALUES conns=$CONNS reqs=$REQS"
echo "L2-MODBUS-TCP VERIFIED" > "$EVDIR/verdict.txt"
