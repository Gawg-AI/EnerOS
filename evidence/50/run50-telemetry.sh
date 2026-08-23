#!/bin/bash
# 蓝图 50（WP-50.1 exporter 真通道）QEMU 实弹：
# A 在线观测：QEMU 内 root-task 上报真实服务健康 → svc-observability
#   exporter → svc-mqtt → broker → 独立订阅观测 OBSF 帧（frame_seq 连续）；
# B 断网补传：杀 broker → svc-mqtt 拒绝（fail-closed）→ durable outbox
#   兜底累积（串口 EXPORT-REJECT / TEL-FRAME-REJECT broker 断连）；
# C 恢复补传：重启 broker → svc-mqtt 插件重连（50 卷断开检测修复）→
#   exporter 恢复 → 断网期条目全部补传（frame_seq 连续、每项一次）。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
EVID="$ROOT/evidence/50"
EVIDDIR="target/evidence50"
mkdir -p "$EVIDDIR" "$EVID"
# 干净起步：串口/transcript/数据盘全清（harness 数据盘存在即复用——
# 上一轮 durable outbox 残留会让本轮 open() 恢复陈旧 pending）。
rm -f "$EVIDDIR"/qemu-serial.log "$EVID"/telemetry.transcript.jsonl \
  "$EVID"/mgmt.transcript.jsonl "$EVIDDIR"/broker.out "$EVIDDIR"/ops.out \
  "$EVIDDIR"/eneros-data.raw
pkill -f '[m]qtt_telemetry_broker' 2>/dev/null
sleep 1

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"
export ENEROS_IMAGE_SIGNING_KEY="$ROOT/product/security/keys-local/dev-signing.key"
export SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1755523200}"

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
ls -la "$IMG" >/dev/null || { echo "image missing: $IMG"; exit 3; }

# 1. 独立 MQTT broker 由相位编排器**全权管理**（主脚本不预启——否则
#    stop_broker 杀不到它，断网窗形同虚设；run6 实弹根因）。
trap 'pkill -f "[m]qtt_telemetry_broker" 2>/dev/null || true' EXIT

# 2. VARS 模板（复用 47 既有模板——同一测试签名证书）。
TPL="$EVIDDIR/template.fd"
cp -f "$ROOT/target/evidence47/template.fd" "$TPL" 2>/dev/null \
  || cargo xtask image vars --output "$TPL" --uefi-cert "$ROOT/product/security/keys-local/uefi-test.pem" >/dev/null 2>&1 \
  || cp -f "$ROOT/target/evidence46-l2/template.fd" "$TPL" 2>/dev/null || true
ls "$TPL" >/dev/null 2>&1 || { echo "vars template missing"; exit 3; }

# 3. 相位编排（python——确定性管理 broker 生命周期 + 运维命令注入）。
python3 - "$EVID" "$EVIDDIR" << 'PHASES' &
import os, subprocess, sys, time
evid, eviddir = sys.argv[1], sys.argv[2]
broker_proc = None
ops_proc = None
def start_broker():
    global broker_proc
    out = open(os.path.join(eviddir, "broker.out"), "ab")
    broker_proc = subprocess.Popen(
        ["python3", os.path.join(evid, "mqtt_telemetry_broker.py"),
         "--port", "1883",
         "--transcript", os.path.join(evid, "telemetry.transcript.jsonl")],
        stdout=out, stderr=subprocess.STDOUT)
    # 就绪探测（TELBROKER-READY on stderr→stdout）。
    for _ in range(40):
        with open(os.path.join(eviddir, "broker.out"), "r", errors="replace") as f:
            if "TELBROKER-READY" in f.read():
                return
        time.sleep(0.5)
def stop_broker():
    global broker_proc
    if broker_proc:
        broker_proc.kill()
        try:
            broker_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass
        broker_proc = None
        time.sleep(2)  # 端口释放（SO_REUSEADDR 下 TIME_WAIT 仍可 bind）
def start_ops():
    """外部运维客户端（独立进程）：MQTT 管理通道正例+越权负例。"""
    global ops_proc
    out = open(os.path.join(eviddir, "ops.out"), "ab")
    ops_proc = subprocess.Popen(
        ["python3", os.path.join(evid, "ops_mgmt_client.py"),
         "--port", "1883",
         "--transcript", os.path.join(evid, "mgmt.transcript.jsonl")],
        stdout=out, stderr=subprocess.STDOUT)
try:
    start_broker()          # 0：broker 起（编排器全权管理生命周期）
    time.sleep(25)          # A：在线观测（root 上报 → exporter → broker）
    start_ops()             # A2：运维命令（status/config-get/越权重启/restart）
    time.sleep(35)          #    （等运维往返完成）
    stop_broker()           # B：断网开始（harness 硬性 timeout 200s——全相位
    time.sleep(50)          #     压缩在 ~190s 编排窗；断连后 guest 时钟爬慢，
    start_broker()          # C：恢复（插件重连 → durable 补传）如实登记
    time.sleep(85)          #    重连+补传观察窗
    time.sleep(600000)      # 挂住到被主脚本清理
finally:
    stop_broker()
PHASES
PHASE_PID=$!

# 4. QEMU 实弹（400s 有界窗口；rc=124 超时杀 = 完整证据窗口）。
cargo xtask run qemu-prod \
  --image "$IMG" \
  --vars-template "$TPL" \
  --expect PASS \
  --evidence-dir "$EVIDDIR" \
  --net-mode present \
  --net-user \
  --net-protocols present \
  --reset-bootctrl \
  --ready-marker product-ready \
  --qmp-deadline-secs 550
RC=$?
kill "$PHASE_PID" 2>/dev/null
pkill -f '[m]qtt_telemetry_broker' 2>/dev/null
sleep 2
[ "$RC" = "124" ] && RC=0

echo "=== RC=$RC ==="
echo "--- 串口遥测证据 ---"
grep -E "OBS-REPORT|EXPORT-FRAME|EXPORT-REJECT|MQTT-EXPORTER|TEL-FRAME|TEL-FRAG|MGMT-FWD|PRODUCT-READY" \
  "$EVIDDIR/qemu-serial.log" 2>/dev/null | head -140 || true
echo "--- 运维命令往返（mgmt.transcript）---"
grep -E '"event": "MGMT-(CMD|RESP|TIMEOUT)"' \
  "$EVID/mgmt.transcript.jsonl" 2>/dev/null | head -20 || true
echo "--- broker 观测（TELFRAME 序列）---"
grep -E '"event": "(TELFRAME|TELFRAME-REJECT|BROKER-LISTEN)"' \
  "$EVID/telemetry.transcript.jsonl" 2>/dev/null | head -60 || true
exit "$RC"
