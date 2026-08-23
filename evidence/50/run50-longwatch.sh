#!/bin/bash
# 蓝图 50（WP-50.1 DoD② 补传恢复段长观察窗）：
# harness Linux 路径硬性 `timeout --signal=KILL 200s`（qmp-deadline 只影响
# QMP 关机等待），断连后 guest 时钟爬慢使恢复段被截断——本脚本以 harness
# 保存的**同一 qemu.argv**（同镜像/同盘/同网参）手动拉长观察窗至 420s，
# 编排相位与 run50-telemetry.sh 一致。证据用途：补传「恢复→不丢」段可视化。
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"
EVID="$ROOT/evidence/50"
EVIDDIR="target/evidence50"
mkdir -p "$EVIDDIR" "$EVID"
rm -f "$EVID"/telemetry.transcript.jsonl "$EVID"/mgmt.transcript.jsonl \
  "$EVIDDIR"/broker.out "$EVIDDIR"/ops.out "$EVIDDIR"/eneros-data.raw
# 数据盘清空重建（16MB 全零；与 harness `fs::write(zeros)` 同语义——
# qemu.argv 直引用该路径，harness 不在时须自备）。
python3 -c "open('target/evidence50/eneros-data.raw','wb').truncate(16*1024*1024)" 2>/dev/null || \
  dd if=/dev/zero of="$EVIDDIR/eneros-data.raw" bs=1M count=16 2>/dev/null
pkill -f '[m]qtt_telemetry_broker' 2>/dev/null
sleep 1

export PATH="$HOME/.cache/xpack-aarch64-none-elf-gcc-15.2.1-1.1/bin:$HOME/.cache/cmake-3.31.6-linux-x86_64/bin:$HOME/.cache/dtc-deb/usr/bin:$HOME/.cache/xmllint-bin/usr/bin:$PATH"
export LD_LIBRARY_PATH="$HOME/.cache/libxml2-deb/usr/lib/x86_64-linux-gnu:$HOME/.cache/libicu74-deb/usr/lib/x86_64-linux-gnu"
export PYTHONPATH="$HOME/.cache/pyfdt-0.3"

# 1. 相位编排（与 telemetry 脚本一致；观察窗 420s）。
python3 - "$EVID" "$EVIDDIR" << 'PHASES' &
import os, subprocess, sys, time
evid, eviddir = sys.argv[1], sys.argv[2]
broker = None
def start_broker():
    global broker
    out = open(os.path.join(eviddir, "broker.out"), "ab")
    broker = subprocess.Popen(
        ["python3", os.path.join(evid, "mqtt_telemetry_broker.py"),
         "--port", "1883",
         "--transcript", os.path.join(evid, "telemetry.transcript.jsonl")],
        stdout=out, stderr=subprocess.STDOUT)
    for _ in range(40):
        with open(os.path.join(eviddir, "broker.out"), errors="replace") as f:
            if "TELBROKER-READY" in f.read(): return
        time.sleep(0.5)
def stop_broker():
    global broker
    if broker:
        broker.kill()
        try: broker.wait(timeout=5)
        except subprocess.TimeoutExpired: pass
        broker = None; time.sleep(2)
try:
    start_broker()
    time.sleep(28)
    subprocess.Popen(["python3", os.path.join(evid, "ops_mgmt_client.py"),
                      "--port", "1883",
                      "--transcript", os.path.join(evid, "mgmt.transcript.jsonl")],
                     stdout=open(os.path.join(eviddir,"ops.out"),"ab"),
                     stderr=subprocess.STDOUT)
    time.sleep(40)
    stop_broker()           # B：断网 @ ~68s
    time.sleep(55)
    start_broker()          # C：恢复 @ ~123s（观察窗至 420s）
    time.sleep(300000)
finally:
    stop_broker()
PHASES
PHASE_PID=$!

# VARS 从模板刷新（harness 同语义——复用被启动过程改写的 run.fd 会
# AnchorMismatch 拒导）+ bootctrl 归零（--reset-bootctrl 等效）。
cp -f "$EVIDDIR/template.fd" "$EVIDDIR/AAVMF_VARS.run.fd"
python3 tools/reset-bootctrl.py "$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw" 2>/dev/null ||   python3 - "$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw" <<'PYRESET'
import sys
sys.path.insert(0, "tools")
import reset_bootctrl as rb
img = sys.argv[1]
data = bytearray(open(img, "rb").read())
rb.reset_bootctrl(data)
open(img, "wb").write(data)
print("bootctrl reset OK")
PYRESET

sleep 8   # 等 QEMU 前的构建检查余量（与 cargo xtask run 启动节奏对齐）

# 2. 同参直跑 QEMU（argv 来自上次 harness 保存；timeout 420s）。
QEMU_ARGS=$(cat "$EVIDDIR/qemu.argv")
# stdbuf -o0：禁用 stdio 块缓冲——SIGKILL 结尾不再丢尾部串口
# （diag 移除后缓冲刷写变稀，此前「卡死」假象实为缓冲丢失）。
stdbuf -o0 -e0 timeout --signal=KILL 420s $QEMU_ARGS > "$EVIDDIR/qemu-serial.log" 2>&1
RC=$?
kill "$PHASE_PID" 2>/dev/null
pkill -f '[m]qtt_telemetry_broker' 2>/dev/null
sleep 2
[ "$RC" = "124" ] && RC=0
echo "=== RC=$RC ==="
grep -cE "TEL-FRAME-ACCEPT|TEL-FRAME-REJECT broker|EXPORT-FRAME|EXPORT-REJECT" "$EVIDDIR/qemu-serial.log"
exit "$RC"
