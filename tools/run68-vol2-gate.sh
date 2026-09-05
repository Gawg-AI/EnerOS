#!/bin/bash
# 蓝图 68 卷 68.2（v0.322.0）出口 QEMU 实弹回归（2026-09-05）：
#   遥测 epoch 多源归位与 104 授予边接通卷：消费侧 per-source epoch 归位
#   （trusted.rs SourceEpochTable + 钉扎门 restore 同步删除——F13 真根因
#   "每服务 floor 各取 boot now_ms + 单全局 epoch 最后钉扎者赢"修复）+
#   iec104→telemetry 授予边接通（62.3 §H 在册项转正）+ master_write u16
#   截断卫生项。root-task 内嵌授予表 → **必走 sel4-build-rust 重链接**；
#   qemu-prod 实弹（modbus + iec104 双对端在位）双窗：
#   窗 A（normal）判据：
#   - PRODUCT-READY: true；零 PANIC；readiness 29 服务基线；
#   - 授予边：EIPC-GRANT-TABLE edges=35；svc-iec104 telemetry EIPC bound
#     ≥1；NOT granted 词形零出现；
#   - 双源 OBS-SOURCE 各自 epoch 正常分配（id=modbus/iec104 两行，
#     epoch 必不相等——F13 前半实弹可见化）；
#   - SNAPSHOT rejected: SourceEpochMismatch = 0；OBS-SHADOW eval reject
#     = 0；agent=OK > 0（AI 链复活，对照调查窗 59→0 翻转）；
#   - iec104 ACQ 行 ≥4（run63 基线）+ svc-iec104 OBS-ROUND sent>0
#     accepted>0（68.2 新上行链路实弹）；
#   - CONTROL-CHAIN dispatch=Committed（决策链无回归）；
#   - modbus 7 点值基线 + OBS-ROUND sent=7 accepted=7（单源行为不变）；
#   - ACQ-ROUND vs 65 基线（±25%，与窗长解耦）；CONTROL-CHAIN/
#     OBS-REPORT 窗 A/B 同形互比（65 基线=单源早停窗结构性不可比，
#     重标基线注明见 §8）；
#   窗 B（restart）：QEMU 强杀后全新 boot——双源重新 open()（新 epoch，
#   per-source max 放行），SNAPSHOT rejected: SourceEpochMismatch 仍 = 0
#   且 agent=OK ≥20（毒化不随重启复发；MemEpochStore 非持久（ADR-0047
#   缺口维持）在全系统重启形态下生产/消费两侧同步重置，如实登记不表现）。
# 证据归档 evidence/68/vol68-2/。脚本建模 tools/run63-vol2-gate.sh（双对端
# 先例）+ tools/run68-vol1-gate.sh（65 基线对照先例）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/68/vol68-2"
mkdir -p "$EVROOT/build"

# build 前导单源（清场/钥导出/compile 收编 gate-env.sh）。
gate_cleanup_env 5597 5602 2404
gate_export_signing_key

kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave6") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5597 5602 2404; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接（root-task 内嵌 EIPC_GRANTS
表——本卷加边必重链接）+ image raw qemu-prod ==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -10 "$EVROOT/build/relink.log"; exit 9; }
grep -a "Rust ELF:" "$EVROOT/build/relink.log" | tail -1
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

run_window() {  # $1=窗口名  $2=deadline secs
  local W="$1" DL="$2"
  local DIR="$ROOT/target/evidence68-vol2/$W"
  mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
  # peer 回放输入按卷拷贝（禁止写已跟踪基线文件，63.1 教训）。
  cp -f "$ROOT/evidence/62/vol62-3/normal.modbus.transcript.jsonl" "$EVROOT/$W.modbus.transcript.jsonl"
  cp -f "$ROOT/evidence/62/vol62-3/normal.iec104.transcript.jsonl" "$EVROOT/$W.iec104.transcript.jsonl"
  python3 "$ROOT/evidence/61/modbus_slave61.py" \
    --transcript "$EVROOT/$W.modbus.transcript.jsonl" &
  local PMOD=$!
  sleep 1
  python3 "$ROOT/evidence/62/iec104_slave62.py" \
    --transcript "$EVROOT/$W.iec104.transcript.jsonl" --deadline "$DL" &
  local P104=$!
  sleep 1
  if ! kill -0 $PMOD 2>/dev/null; then echo "MODBUS-PEER-DIED"; exit 5; fi
  if ! kill -0 $P104 2>/dev/null; then echo "IEC104-PEER-DIED"; exit 5; fi
  cargo xtask run qemu-prod \
    --image "$IMG" \
    --vars-template "$TPL" \
    --expect PASS --evidence-dir "$DIR" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1
  local RRC=$?
  kill $PMOD $P104 2>/dev/null || true
  cp -f "$DIR/qemu-serial.log" "$EVROOT/$W.qemu-serial.log" 2>/dev/null || true
  echo "window $W rc=$RRC（124/2=超时杀=完整窗口）"
  [ -f "$DIR/qemu-serial.log" ] || { echo "serial log missing ($W)"; exit 6; }
}

scan_window() {  # $1=串口日志路径  $2=窗口名
  local S="$1" W="$2"
  echo "=== 窗 $W 出口门判据行扫描 ==="
  local FAIL=0

  # 1. 产品就绪 + 零 PANIC + readiness 29 基线。
  grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }
  local n; n=$(grep -ac "PANIC" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] PANIC = 0"; else echo "[✗] PANIC 出现 $n 次"; FAIL=1; fi
  local RN; RN=$(grep -ac "READINESS: " "$S" 2>/dev/null); RN=${RN:-0}
  if [ "$RN" -eq 29 ]; then echo "[✓] READINESS = 29（服务 ready 基线）"; else echo "[✗] READINESS 行=$RN（基线 29）"; FAIL=1; fi

  # 2. 授予边（WP-68.2.3 接通转正）。
  grep -aq "EIPC-GRANT-TABLE edges=35" "$S" && echo "[✓] EIPC-GRANT-TABLE edges=35（68.2 新边入表）" || { echo "[✗] edges ≠ 35"; FAIL=1; }
  n=$(grep -ac '\[svc-iec104\] telemetry EIPC bound' "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -ge 1 ]; then echo "[✓] svc-iec104 telemetry EIPC bound = $n（边接通）"; else echo "[✗] bound 缺失"; FAIL=1; fi
  n=$(grep -ac 'telemetry EIPC NOT granted' "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] telemetry EIPC NOT granted = 0（缺边词形退场）"; else echo "[✗] NOT granted 出现 $n 次"; FAIL=1; fi

  # 3. 双源 OBS-SOURCE epoch 分配（判据 1；两行 epoch 必不相等——F13 前半）。
  python3 - "$S" <<'PYEOF' || FAIL=1
import re, sys
epochs = {}
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"\[svc-(modbus-tcp|iec104)\] OBS-SOURCE id=\d+ epoch=(\d+) \(config-allocated\)", line)
    if m:
        epochs[m.group(1)] = int(m.group(2))
if set(epochs) != {"modbus-tcp", "iec104"}:
    print(f"[✗] OBS-SOURCE 双源行不齐: {sorted(epochs)}"); sys.exit(1)
a, b = epochs["modbus-tcp"], epochs["iec104"]
if a == b:
    print(f"[✗] 双源 epoch 相等（{a}）——F13 模型不成立"); sys.exit(1)
print(f"[✓] 双源 OBS-SOURCE epoch 独立分配: modbus={a} iec104={b}")
PYEOF

  # 4. 毒化清零族（判据 2/5：normal 与 restart 同判据）。
  n=$(grep -ac "SNAPSHOT rejected: SourceEpochMismatch" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] SNAPSHOT rejected: SourceEpochMismatch = 0（毒化清零）"; else echo "[✗] SourceEpochMismatch 快照拒绝 $n 次"; FAIL=1; fi
  n=$(grep -ac "OBS-SHADOW eval reject" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] OBS-SHADOW eval reject = 0"; else echo "[✗] OBS-SHADOW eval reject 出现 $n 次"; FAIL=1; fi
  # 健康度硬门 ≥20：对照 62/65 基线 59-60/窗；退化形态（=1，首评即被打
  # 死）在 68.2 首轮实弹出现过（ClockNotMonotonic 次生缺陷，执行卡 §D）
  # ——">0" 不足以区分，取基线 1/3 为下界。
  local AGOK; AGOK=$(grep -ac "agent=OK" "$S" 2>/dev/null); AGOK=${AGOK:-0}
  if [ "$AGOK" -ge 20 ]; then echo "[✓] agent=OK 行 = $AGOK（AI 链健康，62/65 基线 59-60）"; else echo "[✗] agent=OK = $AGOK < 20（AI 链非健康）"; FAIL=1; fi

  # 5. iec104 采集与上行（判据 3：ACQ ≥4 维持 + OBS-ROUND 新链路实弹）。
  local ACQ_N; ACQ_N=$(grep -ac '\[svc-iec104\] ACQ point=' "$S" 2>/dev/null); ACQ_N=${ACQ_N:-0}
  if [ "$ACQ_N" -ge 4 ]; then echo "[✓] svc-iec104 ACQ 行 ≥4（run63 基线保持，实得 $ACQ_N）"; else echo "[✗] svc-iec104 ACQ 行不足（$ACQ_N）"; FAIL=1; fi
  local IEC_OK; IEC_OK=$(grep -acE '\[svc-iec104\] OBS-ROUND sent=[1-9][0-9]* accepted=[1-9][0-9]*' "$S" 2>/dev/null); IEC_OK=${IEC_OK:-0}
  if [ "$IEC_OK" -gt 0 ]; then echo "[✓] svc-iec104 OBS-ROUND sent>0 accepted>0 行 = $IEC_OK（上行实弹）"; else echo "[✗] svc-iec104 OBS-ROUND 上行实弹缺失"; FAIL=1; fi

  # 6. 决策链无回归（判据 4）。
  grep -q "dispatch=Committed" "$S" && echo "[✓] CONTROL-CHAIN dispatch=Committed" || { echo "[✗] dispatch=Committed 缺失"; FAIL=1; }

  # 7. modbus 单源行为不变（回归面）。
  python3 - "$S" <<'PYEOF' || FAIL=1
import re, sys
vals = {}
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"\[svc-modbus-tcp\] ACQ point=(\d+) value=([-\d.]+) q=good", line)
    if m:
        vals.setdefault(int(m.group(1)), m.group(2))
baseline = {1: "400", 2: "50", 3: "50", 4: "0.6", 5: "1", 6: "0", 7: "0"}
bad = [(p, vals.get(p), v) for p, v in baseline.items() if vals.get(p) != v]
if bad:
    print(f"[✗] modbus 值基线不一致: {bad}"); sys.exit(1)
print("[✓] modbus 7 点值基线逐点一致（400/50/50/0.6/1/0/0）")
PYEOF
  grep -q '\[svc-modbus-tcp\] OBS-ROUND sent=7 accepted=7' "$S" && echo "[✓] modbus OBS-ROUND sent=7 accepted=7" || { echo "[✗] modbus OBS-ROUND 基线词形缺失"; FAIL=1; }

  # 8. 节奏族（68.2 重标基线并注明）：CONTROL-CHAIN≈轮数、OBS-REPORT≈
  # 上行量，随窗长与形态（双源+AI 活）缩放——原 65 卷基线为单源早停窗
  # （CONTROL-CHAIN 15114 vs 本卷满窗 23706），结构性不可比，退役。
  # 改用同形互比：窗 A vs 窗 B（同镜像独立 boot、同满窗时长，±25% 容差）。
  # ACQ-ROUND 与窗长解耦，另保 65 基线对照（首轮实弹 37 vs 35 已证可比）。
  local BASE="$ROOT/evidence/65/vol65-1/normal.qemu-serial.log"
  local bn nn ok
  bn=$(grep -ac "ACQ-ROUND" "$BASE" 2>/dev/null); bn=${bn:-0}
  nn=$(grep -ac "ACQ-ROUND" "$S" 2>/dev/null); nn=${nn:-0}
  ok=$(python3 -c "print(1 if abs($nn-$bn) <= 0.25*$bn else 0)")
  if [ "$ok" -eq 1 ]; then echo "[✓] ACQ-ROUND vs 65 基线一致（基线=$bn 现卷=$nn，±25%）"; else echo "[✗] ACQ-ROUND vs 65 基线漂移（基线=$bn 现卷=$nn）"; FAIL=1; fi
  if [ "$W" = "restart" ]; then
    BASE="$ROOT/evidence/68/vol68-2/normal.qemu-serial.log"
    for fam in CONTROL-CHAIN OBS-REPORT; do
      bn=$(grep -ac "$fam" "$BASE" 2>/dev/null); bn=${bn:-0}
      nn=$(grep -ac "$fam" "$S" 2>/dev/null); nn=${nn:-0}
      ok=$(python3 -c "print(1 if abs($nn-$bn) <= 0.25*$bn else 0)")
      if [ "$ok" -eq 1 ]; then echo "[✓] $fam 窗间节奏一致（A=$bn B=$nn，±25% 容差）"; else echo "[✗] $fam 窗间节奏漂移（A=$bn B=$nn）"; FAIL=1; fi
    done
  fi

  # 9. 负断言。
  for pat in SUPERVISOR HEARTBEAT-KILL; do
    n=$(grep -ac "$pat" "$S" 2>/dev/null); n=${n:-0}
    if [ "$n" -eq 0 ]; then echo "[✓] $pat = 0"; else echo "[✗] $pat 出现 $n 次（基线 0）"; FAIL=1; fi
  done

  return $FAIL 2>/dev/null || true
}

echo "=== 步骤 3：窗 A normal（modbus + iec104 双对端）==="
run_window normal 600
scan_window "$ROOT/target/evidence68-vol2/normal/qemu-serial.log" normal
FAILA=$?

echo "=== 步骤 4：窗 B restart（强杀 QEMU 后全新 boot——双源重新 open()）==="
# 已知坑（蓝图 60 E.2 / SPEC-E 在册）：qemu-prod 启动确定性冻结（UEFI 期
# 卡死，重跑即过）——restart 窗带一次重试（引导成功判据 = serial 出现
# READINESS 行）。
for ATTEMPT in 1 2; do
  pkill -9 -f "qemu-system" 2>/dev/null || true
  sleep 2
  run_window restart 600
  if grep -aq "READINESS: " "$ROOT/target/evidence68-vol2/restart/qemu-serial.log" 2>/dev/null; then
    break
  fi
  echo "[i] 窗 B 第 $ATTEMPT 次引导未完成（UEFI 冻结坑形态）——重试"
done
scan_window "$ROOT/target/evidence68-vol2/restart/qemu-serial.log" restart
FAILB=$?

echo "=== 窗间对照：重启漂移如实登记（ADR-0047 缺口维持）==="
python3 - "$EVROOT/normal.qemu-serial.log" "$EVROOT/restart.qemu-serial.log" <<'PYEOF' || true
import re, sys
def grab(p):
    out = {}
    for line in open(p, errors="replace"):
        m = re.search(r"\[svc-(modbus-tcp|iec104)\] OBS-SOURCE id=\d+ epoch=(\d+)", line)
        if m:
            out.setdefault(m.group(1), set()).add(int(m.group(2)))
    return out
a, b = grab(sys.argv[1]), grab(sys.argv[2])
print(f"[i] 窗 A epoch 集: { {k: sorted(v) for k, v in a.items()} }")
print(f"[i] 窗 B epoch 集: { {k: sorted(v) for k, v in b.items()} }")
print("[i] MemEpochStore 非持久：全系统重启两侧（生产/消费）同步重置，漂移可见但不毒化（判据 5 已锁零）")
PYEOF

echo "=== 汇总 ==="
[ $FAILA -eq 0 ] || echo "窗 A FAIL"
[ $FAILB -eq 0 ] || echo "窗 B FAIL"
if [ $FAILA -eq 0 ] && [ $FAILB -eq 0 ]; then
  echo "VOL68.2-GATE: PASS"
else
  echo "VOL68.2-GATE: FAIL"; exit 7
fi
