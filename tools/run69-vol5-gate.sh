#!/bin/bash
# 蓝图 69 卷 69.5（v0.330.0）QEMU 实弹窗（2026-09-07）：
#   Q4 观测窗实弹：peer 离线（不启动 planner-peer）→ 超时裁决 →
#      LEASE-ARMED → root 只读探针（DegradeObserve 25/8）连发 5 次
#      AI_DEGRADE_OBSERVE(0x22) → 串口 [svc-ai-worker] DEGRADE-OBS 行
#      armed=true 且 expires_in_ms>0 且 uses=x/100000（M2 四行确认表的
#      最终收口）。
#   Q5 只读性：每个观测轮 5 连读 DEGRADE-OBS 同刻读数逐字段一致
#      （观测零状态接触）；轮间 uses 单调不增、CONTROL-CHAIN 决策链
#      照常 Committed（观测不改判定）。
#   Q6 OTA 主路：OTA-DOWNLOAD verified / trial started / committed 三词形
#      仍在且顺序不变。
#   Q3 词形零漂移：本窗新增串口行形 = 69.4 出口窗 + {DEGRADE-OBS}。
#   Q7 环境稳态：零 CHAIN-KEY-MISSING、零非豁免 PANIC。
#   环境形状 = 69.4 窗同形（modbus/iec104 从站 + MQTT broker），差异仅
#   刻意不启动 planner-peer（降级武装的触发前提）。
#   用法：run69-vol5-gate.sh
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/69/vol69.5"
RUN="$ROOT/target/evidence69-vol5/win"
DL=840
mkdir -p "$EVROOT" "$RUN"

gate_cleanup_env 5577 5582 2404 1883
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5577 5582 2404 1883; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

DIR="$RUN"
mkdir -p "$DIR"
rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"

echo "=== 69.5 实弹窗（$DL s；peer 刻意离线 → 降级武装场景）==="
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$EVROOT/modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$EVROOT/iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1
python3 "$ROOT/evidence/50/mqtt_telemetry_broker.py" --port 1883 \
  --transcript "$EVROOT/broker-transcript.jsonl" >"$RUN/broker.log" 2>&1 &
PBRK=$!
sleep 1
if ! (ss -tln 2>/dev/null | grep -q ":1883 "); then echo "BROKER-NOT-LISTENING"; exit 4; fi
# 注意：刻意不启动 planner-peer——svc-ai-worker 请求超时 → LEASE-ARMED
# → 观测窗的 armed=true 实弹（Q4）。

cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1 &
QEMU_JOB=$!

wait_ready() {
  for _ in $(seq 1 100); do
    grep -q "PRODUCT-READY: true" "$DIR/qemu-serial.log" 2>/dev/null && return 0
    sleep 3
  done
  return 1
}
wait_ready || { echo "PRODUCT-READY 未达"; kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null; exit 6; }
sleep 660  # 武装 + 大量观测轮 + OTA trial/commit 全链样本余量（总窗 840s 级）

kill $QEMU_JOB $PMOD $P104 $PBRK 2>/dev/null || true
sleep 3
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave62") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true

S="$DIR/qemu-serial.log"
[ -f "$S" ] || { echo "serial log missing"; exit 7; }
cp -f "$S" "$EVROOT/qemu-serial.log"
grep -q "PRODUCT-READY: true" "$S" || { echo "PRODUCT-READY 未 true"; exit 10; }

FAIL=0
# ---- Q4：观测窗实弹（DEGRADE-OBS armed=true + 数字判据）-------------
OBS_N=$(grep -c "svc-ai-worker] DEGRADE-OBS " "$S")
echo "[Q4] DEGRADE-OBS 行数=$OBS_N"
[ "$OBS_N" -ge 25 ] || { echo "[✗][Q4] 观测行过少（探针未达）"; FAIL=1; }
ARMED_N=$(grep "svc-ai-worker] DEGRADE-OBS armed=true" "$S" | wc -l)
echo "[Q4] armed=true 观测行=$ARMED_N"
[ "$ARMED_N" -ge 5 ] || { echo "[✗][Q4] 无 armed=true 观测（lease 武装未实弹）"; FAIL=1; }
LEASE_ARMED=$(grep -c "LEASE-ARMED level=D3" "$S")
echo "[Q4] LEASE-ARMED 行=$LEASE_ARMED"
[ "$LEASE_ARMED" -ge 1 ] || { echo "[✗][Q4] 无 LEASE-ARMED（降级武装未触发）"; FAIL=1; }
# armed=true 行的数字判据：expires_in_ms>0、uses=x/100000 且 x<100000。
BAD_EXP=$(grep "svc-ai-worker] DEGRADE-OBS armed=true" "$S" | grep -vc "expires_in_ms=[1-9]")
echo "[Q4] armed=true 但 expires_in_ms=0 的行=$BAD_EXP（判据 0）"
[ "$BAD_EXP" -eq 0 ] || { echo "[✗][Q4] armed=true 行 expires_in_ms=0（窗口字段失真）"; FAIL=1; }
BAD_USES=$(grep "svc-ai-worker] DEGRADE-OBS armed=true" "$S" | grep -vcE "uses=[0-9]{1,5}/100000")
echo "[Q4] armed=true 但 uses 形态非法的行=$BAD_USES（判据 0）"
[ "$BAD_USES" -eq 0 ] || { echo "[✗][Q4] uses 非 x/100000 形态"; FAIL=1; }

# ---- Q5：五连读只读性（burst 内逐字段一致 + 轮间 uses 单调不增）------
python3 - "$S" <<'PYEOF'
import re, sys
path = sys.argv[1]
pat = re.compile(
    r"DEGRADE-OBS armed=(\w+) expired=(\w+) level=(\d+) lease=([0-9a-f]+) "
    r"uses=(\d+)/(\d+) expires_in_ms=(\d+) deadband=\(([-\d.]+),([-\d.]+)\) "
    r"applies=(\d+) frozen_skips=(\d+)")
rows = []
for line in open(path, errors="ignore"):
    m = pat.search(line)
    if m:
        rows.append(m.groups())
if len(rows) % 5 != 0:
    print(f"[✗][Q5] 观测行总数 {len(rows)} 不是 5 的倍数（burst 结构破坏）"); sys.exit(1)
bursts = [rows[i:i+5] for i in range(0, len(rows), 5)]
bad_burst = 0
bad_clock = 0
uses_seq = []
for bi, b in enumerate(bursts):
    # 只读性判据 = 状态字段 burst 内逐字段一致。expires_in_ms 是 now 的
    # 纯函数——root 五连发跨数毫秒合法递减（单调钟），不属状态接触；
    # 单独判 burst 内不逆增。
    key = lambda r: (r[0], r[1], r[2], r[3], r[4], r[5], r[7], r[8], r[9], r[10])
    sig = [key(r) for r in b]
    if len(set(sig)) != 1:
        bad_burst += 1
    clocks = [int(r[6]) for r in b]
    if any(a < b2 for a, b2 in zip(clocks, clocks[1:])):
        bad_clock += 1
    uses_seq.append((b[0][0], int(b[0][4])))
# 轮间单调只在 armed=true 段内判（false→true 跳变 = 武装事件本身）；
# 递减轮 = 兜底授权记账（决策链消费），持平轮 = 观测不记账的证据。
armed_uses = [u for (a, u) in uses_seq if a == "true"]
mono = all(a >= b2 for a, b2 in zip(armed_uses, armed_uses[1:]))
dec = sum(1 for a, b2 in zip(armed_uses, armed_uses[1:]) if a > b2)
same = sum(1 for a, b2 in zip(armed_uses, armed_uses[1:]) if a == b2)
print(f"[Q5] bursts={len(bursts)} 状态字段不一致 burst 数={bad_burst}（判据 0）")
print(f"[Q5] burst 内 expires_in_ms 逆增数={bad_clock}（判据 0——单调钟）")
print(f"[Q5] armed 段 uses 轮间单调不增={'PASS' if mono else 'FAIL'} "
      f"首={armed_uses[0] if armed_uses else '-'} 末={armed_uses[-1] if armed_uses else '-'} "
      f"递减轮={dec}（兜底授权记账）持平轮={same}（观测不记账）")
if bad_burst != 0 or bad_clock != 0 or not mono:
    sys.exit(1)
sys.exit(0)
PYEOF
[ $? -eq 0 ] || { echo "[✗][Q5] 只读性判据 FAIL"; FAIL=1; }

# ---- Q6：OTA 主路三词形仍在且顺序不变 --------------------------------
L1=$(grep -n "OTA-DOWNLOAD: verified via svc-ota" "$S" | head -1 | cut -d: -f1)
L2=$(grep -n "OTA-DOWNLOAD: trial started" "$S" | head -1 | cut -d: -f1)
L3=$(grep -n "OTA-DOWNLOAD: committed" "$S" | head -1 | cut -d: -f1)
echo "[Q6] OTA 三词形行号: download=${L1:-无} trial=${L2:-无} commit=${L3:-无}"
if [ -z "$L1" ] || [ -z "$L2" ] || [ -z "$L3" ]; then
  echo "[✗][Q6] OTA 主路词形缺失"; FAIL=1
elif [ "$L1" -ge "$L2" ] || [ "$L2" -ge "$L3" ]; then
  echo "[✗][Q6] OTA 主路词形顺序漂移"; FAIL=1
fi

# ---- Q4b：决策链端到端（观测探针共存下照常 Committed）----------------
ROUNDS=$(grep -c "CONTROL-CHAIN: round=" "$S")
COMMITTED=$(grep -oE "dispatch=Committed" "$S" | wc -l)
echo "[Q4b] CONTROL-CHAIN rounds=$ROUNDS dispatch=Committed x$COMMITTED（观测共存下决策链不变）"
[ "$ROUNDS" -ge 50 ] || { echo "[✗][Q4b] 决策链轮数异常"; FAIL=1; }
[ "$COMMITTED" -ge 50 ] || { echo "[✗][Q4b] 无足量 Committed"; FAIL=1; }

# ---- Q7：环境稳态 -----------------------------------------------------
CKM=$(grep -c "CHAIN-KEY-MISSING" "$S")
echo "[Q7] CHAIN-KEY-MISSING x$CKM（判据 0）"
[ "$CKM" -eq 0 ] || { echo "[✗][Q7] CHAIN-KEY-MISSING 复现"; FAIL=1; }
PANIC=$(grep -a "PANIC: panicked at" "$S" | grep -vc "alloc.rs" || true)
echo "[Q7] 非豁免 PANIC x$PANIC（判据 0）"
[ "$PANIC" -eq 0 ] || { echo "[✗][Q7] 非豁免 PANIC"; FAIL=1; }

# ---- Q3：词形零漂移（新增行形 = {DEGRADE-OBS}，与 69.4 出口窗对照）----
OLD="$ROOT/evidence/69/vol69.4/qemu-serial.log"
if [ -f "$OLD" ]; then
  python3 - "$S" "$OLD" <<'PYEOF'
import re, sys
def shapes(path):
    # 串口行可能交叠（一条 serial line 载两个服务的前缀）——finditer 全扫描。
    out = set()
    pat = re.compile(r"\[([a-z0-9-]+)\] ([A-Z][A-Z0-9-]*)")
    for line in open(path, errors="ignore"):
        for m in pat.finditer(line):
            out.add(f"[{m.group(1)}] {m.group(2)}")
    return out
new, old = shapes(sys.argv[1]), shapes(sys.argv[2])
extra = new - old
missing = old - new
print(f"[Q3] 新增词形={sorted(extra)}")
print(f"[Q3] 消失词形={sorted(missing)}（判据：空——既有词形零退役）")
ok = extra <= {"[svc-ai-worker] DEGRADE-OBS"} and not missing
sys.exit(0 if ok else 1)
PYEOF
  [ $? -eq 0 ] || { echo "[✗][Q3] 词形漂移"; FAIL=1; }
else
  echo "[Q3] 69.4 出口串口日志缺失——跳过对照（人工核对）"
fi

if [ "$FAIL" -eq 0 ]; then echo "=== VOL69.5 QEMU WINDOW: PASS ==="; else echo "=== VOL69.5 QEMU WINDOW: FAIL ==="; exit 11; fi
