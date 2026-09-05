#!/bin/bash
# 蓝图 68 卷 68.4（v0.324.0）出口 QEMU 实弹回归（2026-09-06）：
#   事件面与快照面接电卷：三冻解冻（SS=StoreSink OBS 事件面 / SFS=
#   StoreFactEventSink 事实面 / SM=SnapshotModule 快照面）——
#   事件面：svc-telemetry lazy-open（首个 OBS 批次），CachedBlocks→
#   RegionScope(DATA_EVENT_*)→BlockEventStore→StoreSink Q0 写透；
#   快照面：SnapshotModule（catalog 惰性装载）+ DATA_SNAP_* LFS 卷
#   blob 存档（8 批窗口）；事实面：svc-market 启动期 open+recover。
#   qemu-prod 实弹（modbus+iec104 双对端；不挂 broker——REG-MQTT-
#   BROKER-HANG 预存冻结，vol3 §D 同口径）双窗：
#   窗 A（normal）判据 = 68.3 全量不回归 + 本卷新词形：
#   - PRODUCT-READY true / PANIC 净值 0（豁免 REG-DISPATCH-HEAP-HIGHROUNDS
#     签名）/ READINESS 29（事件面 lazy-open 不拖编队启动）；
#   - 68.3 磁盘轨：TSDB-DISK-OPEN / TSDB-DISK-COMMIT>0 / DUR≤60s /
#     三退场词形=0；
#   - 事件面在环：EVENT-FACE armed（启动期）→ EVENT-STORE-OPEN（首个
#     OBS 触发）→ EVENT-REPLAY recovered=0 consistent=true（fresh 盘
#     format-on-first）→ EVENT-STORE append n>0 fail=0；
#   - 快照面在环：SNAP-MODULE-READY（catalog 到达）→ SNAP-BUILT ≥1 →
#     SNAP-STORED kept≥1；SNAP-SKIP=0（政策/目录失配即红——诚实判据）；
#   - 事实面在环：FACT-FACE-OPEN consistent=true wm=true；
#     FACT-FACE-UNAVAILABLE=0；
#   - 计数族漂移（WP-68.4.3 判据 3，方向性口径）：减速方向红（< 基线
#     75%——基线 vol68-3：TSDB-DISK-COMMIT=13 / OBS-ROUND=10 / modbus
#     ACQ=70 / agent=OK=463，agent=OK 另守 ±25% 双向带）；多轮次方向
#     对照 vol3 四轮自然带 13-24 批信息记录（run1 两处红 = 带方向性
#     误设，非系统缺陷——订正落本头注与 scan 注释）；
#   窗 B（kill+restart，复用窗 A 数据盘）判据：
#   - 跨窗 RPO=0 精确对账：recovered == 窗 A 追加总数（修剪感知——若
#     窗 A 发生 EVENT-PRUNE 则放宽为 ≤ 并告警，当前几何 112≪518 不触）；
#   - LEARN-STATE restored=1（68.3 学习状态恢复判据保留）；
#   - EVENT-REPLAY recovered>0 consistent=true（事件持久性——本卷核心
#     RPO=0 实弹）；
#   - SNAP-RECOVERED blobs>0（快照 blob 存档跨强杀保持）；
#   - EVENT-STORE append 恢复 ≥1；FACT-FACE-OPEN（事实面 remount 路径）；
#   - 68.3：TSDB-DISK-OPEN points>0 / TSDB-DISK-COMMIT ≥1。
#   WP-68.4.4 certifier 可达性 = host 测试
#   wp6844_certifier_reachable_from_real_snapshot_module_product（结论：
#   可行；见执行卡执行记录）。证据归档 evidence/68/vol68-4/。
#   脚本建模 tools/run68-vol3-gate.sh（双窗+豁免签名+数据盘复用先例）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/68/vol68-4"
mkdir -p "$EVROOT/build"

gate_cleanup_env 5597 5602 2404
gate_export_signing_key

kill -9 $(pgrep -f "modbus_slave61.py") 2>/dev/null || true
kill -9 $(pgrep -f "iec104_slave6") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_broker_b2.py") 2>/dev/null || true
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 2
for PORT in 5597 5602 2404 1883; do
  if (ss -tln 2>/dev/null || netstat -tln 2>/dev/null) | grep -q ":$PORT "; then
    echo "PORT $PORT 仍被占——清理失败"; exit 4
  fi
done

echo "=== 步骤 1：config compile（qemu-prod）==="
gate_compile_config "$EVROOT/build/compile.log"

echo "=== 步骤 2：sel4-build-rust 重链接（事件面/事实面 bin 全新）
       + image raw qemu-prod ==="
bash "$ROOT/third_party/sel4-build-rust.sh" >"$EVROOT/build/relink.log" 2>&1
RC=$?
[ $RC -eq 0 ] || { echo "RELINK FAIL rc=$RC"; tail -20 "$EVROOT/build/relink.log"; exit 9; }
grep -a "Rust ELF:" "$EVROOT/build/relink.log" | tail -1
gate_image_raw qemu-prod "$EVROOT/build/image.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/build/image.sha256"

TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

run_window() {  # $1=窗口名  $2=deadline secs  $3=数据盘来源（fresh|A盘路径）
  local W="$1" DL="$2" DATASRC="$3"
  local DIR="$ROOT/target/evidence68-vol4/$W"
  mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
  if [ "$DATASRC" != "fresh" ] && [ -f "$DATASRC" ]; then
    cp -f "$DATASRC" "$DIR/eneros-data.raw"
  fi
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

scan_window() {  # $1=串口日志  $2=窗口名
  local S="$1" W="$2"
  echo "=== 窗 $W 出口门判据行扫描 ==="
  local FAIL=0
  local n

  # ── 68.3 基线全量保留 ──
  grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }
  local PANIC_ALL PANIC_EXEMPT PANIC_NET
  PANIC_ALL=$(grep -ac "PANIC" "$S" 2>/dev/null); PANIC_ALL=${PANIC_ALL:-0}
  PANIC_EXEMPT=$(grep -ac 'svc-control-dispatch\] PANIC: panicked at.*alloc.rs' "$S" 2>/dev/null); PANIC_EXEMPT=${PANIC_EXEMPT:-0}
  PANIC_NET=$((PANIC_ALL - PANIC_EXEMPT))
  if [ "$PANIC_NET" -eq 0 ]; then
    echo "[✓] PANIC 净值 = 0（全量 $PANIC_ALL，豁免签名 $PANIC_EXEMPT）"
  else
    echo "[✗] PANIC 净值 = $PANIC_NET（非豁免 PANIC 出现）"; FAIL=1
  fi
  local RN; RN=$(grep -ac "READINESS: " "$S" 2>/dev/null); RN=${RN:-0}
  if [ "$RN" -eq 29 ]; then echo "[✓] READINESS = 29（事件面 lazy-open 不拖编队）"; else echo "[✗] READINESS 行=$RN（基线 29）"; FAIL=1; fi

  grep -aq "TSDB-DISK-OPEN " "$S" && echo "[✓] TSDB-DISK-OPEN 在" || { echo "[✗] TSDB-DISK-OPEN 缺失"; FAIL=1; }
  n=$(grep -ac "TSDB-DISK-UNAVAILABLE\|TSDB-DISK DISABLED" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] TSDB 降级/停用词形 = 0"; else echo "[✗] TSDB 降级词形 $n 次"; FAIL=1; fi
  n=$(grep -ac "TSDB-DISK-COMMIT " "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -ge 1 ]; then echo "[✓] TSDB-DISK-COMMIT = $n"; else echo "[✗] TSDB-DISK-COMMIT = 0"; FAIL=1; fi
  if grep -aq "TSDB-CHECKPOINT-DUR" "$S"; then
    local MAXDUR; MAXDUR=$(grep -o 'TSDB-CHECKPOINT-DUR ms=[0-9]*' "$S" | grep -o '[0-9]*$' | sort -n | tail -1)
    MAXDUR=${MAXDUR:-0}
    if [ "$MAXDUR" -le 60000 ]; then
      echo "[✓] TSDB-CHECKPOINT-DUR max=${MAXDUR}ms（≤60s）"
    else
      echo "[✗] CHECKPOINT-DUR max=${MAXDUR}ms 超 60s"; FAIL=1
    fi
  else
    echo "[✗] TSDB-CHECKPOINT-DUR 词形缺失"; FAIL=1
  fi

  # ── 68.4 事件面在环 ──
  grep -aq "EVENT-FACE armed" "$S" && echo "[✓] EVENT-FACE armed（启动期武装）" || { echo "[✗] EVENT-FACE armed 缺失"; FAIL=1; }
  grep -aq "\[svc-telemetry\] EVENT-STORE-OPEN base_block=" "$S" && echo "[✓] EVENT-STORE-OPEN（首个 OBS lazy-open）" || { echo "[✗] EVENT-STORE-OPEN 缺失"; FAIL=1; }
  n=$(grep -ac "EVENT-STORE-UNAVAILABLE\|EVENT-STORE-NOT-GRANTED\|SNAP-STORE-UNAVAILABLE" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] 事件/快照面降级词形 = 0"; else echo "[✗] 降级词形 $n 次"; FAIL=1; fi
  if grep -aq "EVENT-REPLAY recovered=" "$S"; then
    local REP CONS
    REP=$(grep -o 'EVENT-REPLAY recovered=[0-9]*' "$S" | tail -1 | grep -o '[0-9]*$')
    CONS=$(grep -o 'EVENT-REPLAY recovered=[0-9]* consistent=[a-z]*' "$S" | tail -1 | grep -o 'consistent=[a-z]*$' | cut -d= -f2)
    if [ "$CONS" = "true" ]; then
      echo "[✓] EVENT-REPLAY recovered=$REP consistent=true"
    else
      echo "[✗] EVENT-REPLAY recovered=$REP consistent=$CONS（扫描发现损坏段）"; FAIL=1
    fi
  else
    echo "[✗] EVENT-REPLAY 词形缺失"; FAIL=1
  fi
  n=$(grep -o 'EVENT-STORE append n=[0-9]* fail=[0-9]*' "$S" | grep -c "fail=0")
  local APPALL; APPALL=$(grep -ac "EVENT-STORE append n=" "$S" 2>/dev/null); APPALL=${APPALL:-0}
  if [ "$APPALL" -ge 1 ] && [ "$n" -eq "$APPALL" ]; then
    echo "[✓] EVENT-STORE append 批=$APPALL 全部 fail=0（n 总和见日志）"
  else
    echo "[✗] EVENT-STORE append 批=$APPALL，fail=0 批=$n（出现追加失败）"; FAIL=1
  fi
  local APPEV; APPEV=$(grep -o 'EVENT-STORE append n=[0-9]*' "$S" | grep -o '[0-9]*$' | paste -sd+ | bc)
  APPEV=${APPEV:-0}
  echo "    [i] 窗口追加事件总数 n=$APPEV"

  # ── 68.4 快照面在环 ──
  grep -aq "SNAP-MODULE-READY" "$S" && echo "[✓] SNAP-MODULE-READY（catalog 装载）" || { echo "[✗] SNAP-MODULE-READY 缺失（catalog 未到达？）"; FAIL=1; }
  n=$(grep -ac "SNAP-BUILT " "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -ge 1 ]; then echo "[✓] SNAP-BUILT = $n（8 批窗口实弹）"; else echo "[✗] SNAP-BUILT = 0"; FAIL=1; fi
  n=$(grep -ac "SNAP-STORED kept=" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -ge 1 ]; then echo "[✓] SNAP-STORED = $n"; else echo "[✗] SNAP-STORED = 0"; FAIL=1; fi
  n=$(grep -ac "SNAP-SKIP" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] SNAP-SKIP = 0（政策/目录/存储零失配）"; else echo "[✗] SNAP-SKIP $n 次——诚实失败面暴露:"; grep -a "SNAP-SKIP" "$S" | sort | uniq -c | head -5; FAIL=1; fi
  n=$(grep -ac "EVENT-PRUNE" "$S" 2>/dev/null); n=${n:-0}
  echo "    [i] EVENT-PRUNE 修剪批数 = $n（滚动保留观测；600s 窗允许 0）"

  # ── 68.4 事实面在环 ──
  grep -aq "FACT-FACE-OPEN base_block=24968" "$S" || { echo "[✗] FACT-FACE-OPEN 缺失（svc-market 事实面未开）"; FAIL=1; }
  if grep -aq "FACT-FACE-OPEN base_block=24968" "$S"; then
    local FCONS FWM
    FCONS=$(grep -o 'FACT-FACE-OPEN base_block=24968 region_blocks_512=256 recovered=[0-9]* consistent=[a-z]* wm=[a-z]*' "$S" | tail -1 | sed 's/.*consistent=//' | cut -d' ' -f1)
    FWM=$(grep -o 'FACT-FACE-OPEN base_block=24968 region_blocks_512=256 recovered=[0-9]* consistent=[a-z]* wm=[a-z]*' "$S" | tail -1 | sed 's/.*wm=//')
    if [ "$FCONS" = "true" ] && [ "$FWM" = "true" ]; then
      echo "[✓] FACT-FACE-OPEN consistent=true wm=true（SFS 解冻实弹）"
    else
      echo "[✗] FACT-FACE-OPEN consistent=$FCONS wm=$FWM"; FAIL=1
    fi
  fi
  n=$(grep -ac "FACT-FACE-UNAVAILABLE" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] FACT-FACE-UNAVAILABLE = 0"; else echo "[✗] 事实面降级 $n 次"; FAIL=1; fi

  # ── 决策链无回归 + 计数族漂移 ±25%（对照 vol68-3 基线）──
  grep -q "dispatch=Committed" "$S" && echo "[✓] CONTROL-CHAIN dispatch=Committed" || { echo "[✗] dispatch=Committed 缺失"; FAIL=1; }
  n=$(grep -ac "SNAPSHOT rejected: SourceEpochMismatch" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] SNAPSHOT SourceEpochMismatch = 0"; else echo "[✗] SourceEpochMismatch $n 次"; FAIL=1; fi
  n=$(grep -ac "\[svc-modbus-tcp\] ACQ point=" "$S" 2>/dev/null); n=${n:-0}
  local ALO; ALO=$((70 * 75 / 100))
  if [ "$n" -ge "$ALO" ]; then
    echo "[✓] modbus ACQ 行 = $n（≥ 基线 75% = $ALO，ACQ 族不减速）"
  else
    echo "[✗] modbus ACQ 行 = $n < $ALO——采集减速疑点"; FAIL=1
  fi
  local AGOK; AGOK=$(grep -ac "agent=OK" "$S" 2>/dev/null); AGOK=${AGOK:-0}
  local AGLO AGHI; AGLO=$((463 * 75 / 100)); AGHI=$((463 * 125 / 100))
  if [ "$AGOK" -ge 20 ] && [ "$AGOK" -ge "$AGLO" ] && [ "$AGOK" -le "$AGHI" ]; then
    echo "[✓] agent=OK = $AGOK（基线 463，±25% 带内 [$AGLO,$AGHI]，下界 20）"
  else
    echo "[✗] agent=OK = $AGOK（带 [$AGLO,$AGHI]）——事件面 duty 疑似饿决策链"; FAIL=1
  fi
  # 计数族漂移判据（WP-68.4.3 判据 3，方向性订正 2026-09-06）：判据保护
  # 的是"事件面入环不得拖慢采集/决策"——**减速方向**（< 基线 75%）红；
  # 多轮次方向不红，对照 vol3 四轮 gate 的自然波动带（13-24 批，68.3
  # 执行卡 D.4 / CHECKPOINT_EVERY_BATCHES 文档在案）信息记录。窗 B 用
  # 自身恢复值作参照（restart run2 实测 15/11 在 ±25% 带内佐证带成立）。
  local TSDC; TSDC=$(grep -ac "TSDB-DISK-COMMIT " "$S" 2>/dev/null); TSDC=${TSDC:-0}
  local TLO; TLO=$((13 * 75 / 100))
  if [ "$TSDC" -ge "$TLO" ]; then
    echo "[✓] TSDB-DISK-COMMIT 批数 = $TSDC（≥ 基线 75% = $TLO；vol3 自然带 13-24）"
  else
    echo "[✗] TSDB-DISK-COMMIT 批数 = $TSDC < $TLO——采集减速疑点"; FAIL=1
  fi
  local OBSR; OBSR=$(grep -ac "OBS-ROUND sent=7 accepted=7" "$S" 2>/dev/null); OBSR=${OBSR:-0}
  local OLO; OLO=$((10 * 75 / 100))
  if [ "$OBSR" -ge "$OLO" ]; then
    echo "[✓] OBS-ROUND = $OBSR（≥ 基线 75% = $OLO）"
  else
    echo "[✗] OBS-ROUND = $OBSR < $OLO——上行减速疑点"; FAIL=1
  fi
  grep -q '\[svc-modbus-tcp\] OBS-ROUND sent=7 accepted=7' "$S" && echo "[✓] modbus OBS-ROUND 基线词形在" || { echo "[✗] OBS-ROUND 词形缺失"; FAIL=1; }

  return $FAIL 2>/dev/null || true
}

echo "=== 步骤 3：窗 A normal（modbus+iec104 双对端）==="
run_window normal 600 fresh
scan_window "$ROOT/target/evidence68-vol4/normal/qemu-serial.log" normal
FAILA=$?

echo "=== 步骤 4：窗 B kill+restart（复用窗 A 数据盘——持久性判据）==="
ADISK="$ROOT/target/evidence68-vol4/normal/eneros-data.raw"
[ -f "$ADISK" ] || { echo "窗 A 数据盘缺失"; exit 8; }
for ATTEMPT in 1 2; do
  pkill -9 -f "qemu-system" 2>/dev/null || true
  sleep 2
  run_window restart 600 "$ADISK"
  if grep -aq "READINESS: " "$ROOT/target/evidence68-vol4/restart/qemu-serial.log" 2>/dev/null; then
    break
  fi
  echo "[i] 窗 B 第 $ATTEMPT 次引导未完成（UEFI 冻结坑形态）——重试（数据盘复用不变）"
done
scan_window "$ROOT/target/evidence68-vol4/restart/qemu-serial.log" restart
FAILB=$?

echo "=== 窗 B 专项：事件/快照持久性（本卷核心判据；跨窗 RPO=0 精确对账）==="
ASUM=$(grep -o 'EVENT-STORE append n=[0-9]*' "$ROOT/target/evidence68-vol4/normal/qemu-serial.log" | grep -o '[0-9]*$' | paste -sd+ | bc)
ASUM=${ASUM:-0}
APRUNE=$(grep -ac "EVENT-PRUNE" "$ROOT/target/evidence68-vol4/normal/qemu-serial.log" 2>/dev/null); APRUNE=${APRUNE:-0}
python3 - "$ROOT/target/evidence68-vol4/restart/qemu-serial.log" "$ASUM" "$APRUNE" <<'PYEOF' || FAILB=1
import re, sys
rep = None; cons = None; blobs = None; pts = None; appended = False
want = int(sys.argv[2])
pruned = int(sys.argv[3])
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"EVENT-REPLAY recovered=(\d+) consistent=(\w+)", line)
    if m:
        rep = int(m.group(1)); cons = m.group(2)
    m = re.search(r"SNAP-RECOVERED blobs=(\d+)", line)
    if m:
        blobs = int(m.group(1))
    m = re.search(r"TSDB-DISK-OPEN base_block=\d+ region_blocks_512=\d+ points=(\d+)", line)
    if m:
        pts = int(m.group(1))
    if "EVENT-STORE append n=" in line:
        appended = True
ok = True
if rep is None or cons != "true":
    print(f"[✗] EVENT-REPLAY 缺失或 inconsistent（rep={rep} cons={cons}）"); ok = False
elif pruned > 0 and rep <= want:
    print(f"[i] 窗 A 发生 EVENT-PRUNE ×{pruned}——对账放宽：recovered={rep} ≤ 追加 {want}（滚动保留语义，见执行卡）")
    print(f"[✓] EVENT-REPLAY recovered={rep} consistent=true")
elif want > 0 and rep != want:
    print(f"[✗] RPO=0 精确对账失守：窗 A 追加 {want}，重启恢复 {rep}（丢 {want-rep}）"); ok = False
else:
    print(f"[✓] 重启后 EVENT-REPLAY recovered={rep} consistent=true（窗 A 追加 {want}，跨窗精确对账零丢失）")
if blobs is None or blobs <= 0:
    print(f"[✗] SNAP-RECOVERED blobs={blobs}——快照 blob 未跨强杀保持"); ok = False
else:
    print(f"[✓] 重启后 SNAP-RECOVERED blobs={blobs}（快照面恢复实弹）")
if pts is None or pts <= 0:
    print(f"[✗] TSDB-DISK-OPEN points={pts}（68.3 判据回归）"); ok = False
else:
    print(f"[✓] TSDB-DISK-OPEN points={pts}（68.3 数据保持不回归）")
import subprocess as _sp
learn = open(sys.argv[1], errors="replace").read()
if "LEARN-STATE restored=1" not in learn:
    print("[✗] LEARN-STATE restored=1 缺失（68.3 学习状态恢复判据）"); ok = False
else:
    print("[✓] LEARN-STATE restored=1（学习状态恢复不回归）")
if not appended:
    print("[✗] 窗 B 无 EVENT-STORE append——事件面未恢复在环"); ok = False
else:
    print("[✓] 窗 B 事件面恢复追加（offset 续接）")
sys.exit(0 if ok else 1)
PYEOF

echo "=== 汇总 ==="
[ $FAILA -eq 0 ] || echo "窗 A FAIL"
[ $FAILB -eq 0 ] || echo "窗 B FAIL"
if [ $FAILA -eq 0 ] && [ $FAILB -eq 0 ]; then
  echo "VOL68.4-GATE: PASS"
else
  echo "VOL68.4-GATE: FAIL"; exit 7
fi
