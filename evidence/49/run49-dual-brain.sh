#!/bin/bash
# 49 卷 WP-49.2 Exit 实弹：远程 dual-brain 三态切换（ADR-0045 方案 B）。
#
# A 远程提案链：peer(soc) 应答 → ai-worker 全量校验 → System2 提案 →
#   dispatch Committed + AUDIT-AI 入链；
# B 越权负例：peer(overbudget) 提案超预算 → 契约层拒绝（bad 计数）→ 降级；
# C 断链降级：杀 broker → 连续超时 ≥3 → PeerUnhealthy（System1 接管）；
# D 恢复回 AI：重启 broker+peer(soc) → 探测应答 → PeerRecovered → 回 System2。
#
# 证据：QEMU 串口（AI-PEER-CFG/AI-RELAY/REMOTE-BRAIN/PEER-MSG/AUDIT-AI/
#       Committed）+ peer/broker transcript（REQUEST/RESPONDED/PUBLISH）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
EVID="$ROOT/evidence/49"
EVIDDIR="target/evidence49"
mkdir -p "$EVIDDIR" "$EVID"
rm -f "$EVIDDIR"/qemu-serial.log "$EVIDDIR"/peer-*.out "$EVIDDIR"/broker.out \
  "$EVID"/*.transcript.jsonl
# 残留进程清场（前轮 broker/peer 存活会让本轮 peer 连错对象——第七轮实弹根因）。
pkill -f '[m]qtt_broker49' 2>/dev/null
pkill -f '[p]eer_brain49' 2>/dev/null
sleep 1

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
ls -la "$IMG" >/dev/null || { echo "image missing"; exit 3; }

BROKER_TRANS="$EVID/broker.transcript.jsonl"
PEER_TRANS="$EVID/peer.transcript.jsonl"
start_peer() { # $1=policy $2=tag
  python3 "$EVID/peer_brain49.py" --port 1883 --policy "$1" \
    --transcript "$PEER_TRANS" >>"$EVIDDIR/peer-$2.out" 2>&1 &
}

# ---- Phase A：broker + soc peer 先起（远程提案正向链）。----
python3 "$EVID/mqtt_broker49.py" --port 1883 --transcript "$BROKER_TRANS" \
  >"$EVIDDIR/broker.out" 2>&1 &
P_BROKER=$!
start_peer soc a
# peer 就绪探测（连上 broker 并 SUBACK 后才算就绪）。
for _ in $(seq 1 20); do
  grep -q PEER-READY "$EVIDDIR/peer-a.out" 2>/dev/null && break
  sleep 0.5
done
# 外部 modbus 从站（独立进程）——采集链真实数据源（47 卷先例；无从站 =
# 遥测 fail-closed 拒收 = 全链 NOACTION，49 首轮实弹教训）。
python3 "$EVID/../47/modbus_slave47.py" --transcript /dev/null >"$EVIDDIR/modbus.out" 2>&1 &
sleep 2

# ---- 相位编排（后台子 shell；按秒推进）。----
# 注：C 相只停 peer（System2=站端 LLM 服务不可用）不杀 broker——
# smoltcp TcpStreamHandle 无重拨 seam（46 卷句柄移交边界），broker 重启
# 场景的传输层重连登记为后续卷项；蓝图「peer 断链→降级→恢复」语义由
# LLM 服务生命周期承载。
# 相位编排（python 编排器——bash sleep/pkill 子壳在第七~十四轮实弹中
# 反复出现 peer 未启动/未连接竞态，改由 subprocess 确定性管理）。
python3 - "$EVID" << 'PHASES' &
import os, signal, subprocess, sys, time
evid = sys.argv[1]
peer = None
def start(policy, tag):
    global peer
    out = open(os.path.join(evid, f"..", "49", f"peer-{tag}.out"), "ab")
    peer = subprocess.Popen(
        ["python3", os.path.join(evid, "peer_brain49.py"),
         "--port", "1883", "--policy", policy,
         "--transcript", os.path.join(evid, "peer.transcript.jsonl")],
        stdout=out, stderr=subprocess.STDOUT)
def stop():
    global peer
    if peer:
        peer.terminate()
        try:
            peer.wait(timeout=5)
        except subprocess.TimeoutExpired:
            peer.kill()
        peer = None
try:
    time.sleep(90)          # A：soc 策略正向链
    stop(); start("overbudget", "b")   # B：越权负例
    time.sleep(75)
    stop()                  # C：System2 断供（连续超时→Unhealthy）
    time.sleep(130)
    start("soc", "d")       # D：恢复探测→回 AI
    time.sleep(10000)       # 挂住到被主脚本清理
finally:
    stop()
PHASES
PHASE_PID=$!
PHASE_PID=$!

# ---- QEMU 实弹（460s 有界窗口；rc=124 超时杀 = 完整证据窗口）。----
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template target/evidence47/template.fd \
  --expect PASS \
  --evidence-dir "$EVIDDIR" \
  --net-mode present \
  --net-user \
  --net-protocols present \
  --reset-bootctrl \
  --qmp-deadline-secs 720
RC=$?
pkill -f '[p]eer_brain49' 2>/dev/null
kill "$PHASE_PID" 2>/dev/null
pkill -f '[m]qtt_broker49' 2>/dev/null
sleep 2
[ "$RC" = "124" ] && RC=0

echo "=== RC=$RC ==="
echo "--- 串口 AI 链证据 ---"
grep -E "AI-PEER-CFG|backend=|AI-RELAY|AI-REQ|AI-RESP|PEER-MSG|REMOTE-BRAIN|AUDIT-AI|Committed|tick=" \
  "$EVIDDIR/qemu-serial.log" 2>/dev/null | head -100 || true
exit "$RC"
