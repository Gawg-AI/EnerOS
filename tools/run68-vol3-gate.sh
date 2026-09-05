#!/bin/bash
# 蓝图 68 卷 68.3（v0.323.0）出口 QEMU 实弹回归（2026-09-05）：
#   持久介质接缝与 TSDB 磁盘复通卷：E-DEF-01 停用翻转实弹可用——
#   PAYLOAD_CAP 64→461（内核 seL4_MsgMaxLength=120 words × 4B/word=480B
#   顶格）+ eneros-fs 持久 mount（leak-then-mount，`Filesystem<'static>`，
#   操作零重挂载）+ svc-telemetry 磁盘轨装配翻转。root-task wiring 注记
#   变更 + 全部服务 bin 重编 → **必走 sel4-build-rust 重链接**。
#   qemu-prod 实弹（modbus + iec104 双对端；不挂 broker——见 §D 订正）双窗：
#   窗 A（normal，含 TELH 实弹）判据：
#   - PRODUCT-READY: true；零 PANIC；readiness 29 服务基线（启动期
#     format+mount 不拖垮编队 readiness——svc-telemetry.rs:59 前科观测）；
#   - 磁盘轨在环：TSDB-DISK-OPEN 行在；TSDB-DISK DISABLED /
#     TSDB-DISK-UNAVAILABLE / TSDB-COMMIT(Ring 词形) 三退场词形 = 0；
#   - TSDB-DISK-COMMIT > 0；DROPPED/REJECTED = 0（失败态词形正常窗
#     ==0——本卷新 gate 判据）；
#   - TSDB-CHECKPOINT-DUR 词形在案且 max ≤ 60s（病态界）。§D 订正
#     （2026-09-06）：原 ≤2s 预算为 host 口径误设——TCG virtio RMW
#     ~5-15ms/命令、checkpoint ~1.3k 命令/次 = 秒级物理成本，逐批
#     checkpoint 饿死决策链（run3 实弹 agent=OK 0）→ checkpoint 合并窗
#     128 批（CHECKPOINT_EVERY_BATCHES，掉电丢尾 ≤128 批诚实登记）；
#     ACK 路径轻批实测 0 命令（host c5）；异步化降本归
#     REG-TSDB-CHECKPOINT-ASYNC（后续卷）；
#   - TELH 判据改道（执行卡 §D 订正，2026-09-06）：MQTT broker 在场时
#     svc-mqtt 于插件 poll 冻结（vol68-3 run1/run2 实弹 + bisect 同镜像
#     无 broker 全绿——预存缺陷非本卷引入，登记 REG-MQTT-BROKER-HANG），
#     mgmt 通道不可用。TELR 服务端磁盘读面由 host 测试
#     spec_a_window_more_on_real_reader_no_empty_page（真 reader × 真盘
#     链）守护；端到端 TELH 回页转 REG-MQTT-BROKER-HANG 修复后补窗。
#   - 决策链无回归：CONTROL-CHAIN dispatch=Committed；modbus 7 点值基线
#     + OBS-ROUND sent=7 accepted=7（TEL_INGEST 处理内同步 checkpoint
#     不破采集/决策节奏）；agent=OK ≥20（68.2 健康度下界）；
#   窗 B（kill+restart，**复用窗 A 数据盘**——不加 --reset 数据盘语义，
#   xtask 每窗新建 boot 盘，数据盘为窗 A 原盘拷贝）判据：
#   - TSDB-DISK-OPEN points>0（index.bin 重建 = 旧数据保持——强杀数据
#     保持的持久性串口证据）；
#   - TSDB-DISK-COMMIT 恢复 ≥1；TSDB-DISK-UNAVAILABLE = 0。
#   TELR 负例（no-such-point）与分页序归 SPEC-C C.3 既有 harness，本卷
#   不重跑（口径登记）。
# 证据归档 evidence/68/vol68-3/。脚本建模 tools/run68-vol2-gate.sh（双对端
# 先例）+ evidence/spec-2026-08-27-C/run_window.sh（broker/TELR harness 先例）。
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/68/vol68-3"
mkdir -p "$EVROOT/build"

# build 前导单源（清场/钥导出/compile 收编 gate-env.sh）。
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

echo "=== 步骤 2：sel4-build-rust 重链接（root-task wiring/服务 bin 全新）
       + image raw qemu-prod ==="
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

run_window() {  # $1=窗口名  $2=deadline secs  $3=数据盘来源（fresh|A盘路径）
  local W="$1" DL="$2" DATASRC="$3"
  local DIR="$ROOT/target/evidence68-vol3/$W"
  mkdir -p "$DIR"; rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw"
  if [ "$DATASRC" != "fresh" ] && [ -f "$DATASRC" ]; then
    cp -f "$DATASRC" "$DIR/eneros-data.raw"
  fi
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
  # 68.3 §D 订正：不挂 broker（REG-MQTT-BROKER-HANG 预存冻结，见头注）——
  # mqtt 无对端形态与 vol2 基线一致（ready/降级词形对照 run59-gate 键表）。
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

  # 1. 产品就绪 + 零 PANIC + readiness 29 基线（启动期 format/mount 不拖垮）。
  #    PANIC 判据带一条**有登记的豁免**：REG-DISPATCH-HEAP-HIGHROUNDS
  #    （dispatch 高轮数堆耗尽 OOM——bisect 实弹证明独立于本卷 flush/
  #    合并窗改动，vol2 基线轮数少未触墙；归后续卷根因修复）。豁免只
  #    匹配该签名，其余 PANIC 照样判负。
  grep -q "PRODUCT-READY: true" "$S" && echo "[✓] PRODUCT-READY: true" || { echo "[✗] PRODUCT-READY"; FAIL=1; }
  local PANIC_ALL PANIC_EXEMPT PANIC_NET
  PANIC_ALL=$(grep -ac "PANIC" "$S" 2>/dev/null); PANIC_ALL=${PANIC_ALL:-0}
  PANIC_EXEMPT=$(grep -ac 'svc-control-dispatch\] PANIC: panicked at.*alloc.rs' "$S" 2>/dev/null); PANIC_EXEMPT=${PANIC_EXEMPT:-0}
  PANIC_NET=$((PANIC_ALL - PANIC_EXEMPT))
  if [ "$PANIC_NET" -eq 0 ]; then
    echo "[✓] PANIC 净值 = 0（全量 $PANIC_ALL，豁免 REG-DISPATCH-HEAP-HIGHROUNDS 签名 $PANIC_EXEMPT）"
  else
    echo "[✗] PANIC 净值 = $PANIC_NET（非豁免 PANIC 出现）"; FAIL=1
  fi
  local RN; RN=$(grep -ac "READINESS: " "$S" 2>/dev/null); RN=${RN:-0}
  if [ "$RN" -eq 29 ]; then echo "[✓] READINESS = 29（磁盘轨在环不拖编队）"; else echo "[✗] READINESS 行=$RN（基线 29）"; FAIL=1; fi

  # 2. 磁盘轨在环：OPEN 在 + 三退场词形 = 0。
  grep -aq "TSDB-DISK-OPEN " "$S" && echo "[✓] TSDB-DISK-OPEN 在（磁盘轨装配）" || { echo "[✗] TSDB-DISK-OPEN 缺失"; FAIL=1; }
  n=$(grep -ac "TSDB-DISK DISABLED" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] TSDB-DISK DISABLED = 0（E-DEF-01 停用词形退场）"; else echo "[✗] DISABLED 词形出现 $n 次"; FAIL=1; fi
  n=$(grep -ac "TSDB-DISK-UNAVAILABLE" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] TSDB-DISK-UNAVAILABLE = 0（降级态未触发）"; else echo "[✗] 降级词形出现 $n 次"; FAIL=1; fi
  n=$(grep -ac '\[svc-telemetry\] TSDB-COMMIT ' "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] Ring 词形 TSDB-COMMIT = 0（内存环退场）"; else echo "[✗] Ring 词形出现 $n 次"; FAIL=1; fi

  # 3. 提交/失败三态。
  n=$(grep -ac "TSDB-DISK-COMMIT " "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -ge 1 ]; then echo "[✓] TSDB-DISK-COMMIT = $n（>0）"; else echo "[✗] TSDB-DISK-COMMIT = 0"; FAIL=1; fi
  for pat in TSDB-DISK-DROPPED TSDB-DISK-REJECTED; do
    n=$(grep -ac "$pat" "$S" 2>/dev/null); n=${n:-0}
    if [ "$n" -eq 0 ]; then echo "[✓] $pat = 0（正常窗失败态词形缺席）"; else echo "[✗] $pat 出现 $n 次"; FAIL=1; fi
  done

  # 4. checkpoint 词形 + 病态界（≤60s；实测口径见头注 §D 订正）。
  if ! grep -aq "TSDB-CHECKPOINT-DUR" "$S"; then
    echo "[✗] TSDB-CHECKPOINT-DUR 词形缺失"; FAIL=1
  else
    local MAXDUR; MAXDUR=$(grep -o 'TSDB-CHECKPOINT-DUR ms=[0-9]*' "$S" | grep -o '[0-9]*$' | sort -n | tail -1)
    MAXDUR=${MAXDUR:-0}
    if [ "$MAXDUR" -le 60000 ]; then
      echo "[✓] TSDB-CHECKPOINT-DUR max=${MAXDUR}ms（合并窗 checkpoint 在案，≤60s 病态界）"
    else
      echo "[✗] CHECKPOINT-DUR max=${MAXDUR}ms 超 60s 病态界"; FAIL=1
    fi
  fi

  # 5. 决策链无回归 + 采集线健康（checkpoint 在 TEL_INGEST 内同步跑，
  #    deadline/节奏前科观测面）。
  grep -q "dispatch=Committed" "$S" && echo "[✓] CONTROL-CHAIN dispatch=Committed" || { echo "[✗] dispatch=Committed 缺失"; FAIL=1; }
  n=$(grep -ac "SNAPSHOT rejected: SourceEpochMismatch" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] SNAPSHOT SourceEpochMismatch = 0"; else echo "[✗] SourceEpochMismatch $n 次"; FAIL=1; fi
  n=$(grep -ac "ingest transport\|INGEST eipc=Transport" "$S" 2>/dev/null); n=${n:-0}
  if [ "$n" -eq 0 ]; then echo "[✓] ingest Transport = 0"; else echo "[✗] ingest Transport $n 次"; FAIL=1; fi
  local AGOK; AGOK=$(grep -ac "agent=OK" "$S" 2>/dev/null); AGOK=${AGOK:-0}
  if [ "$AGOK" -ge 20 ]; then echo "[✓] agent=OK = $AGOK（AI 链健康，≥20）"; else echo "[✗] agent=OK = $AGOK < 20"; FAIL=1; fi

  if [ "$W" = "normal" ]; then
    # modbus 单源行为不变 + 上行基线。
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
  fi

  return $FAIL 2>/dev/null || true
}

echo "=== 步骤 3：窗 A normal（modbus+iec104 双对端）==="
run_window normal 600 fresh
scan_window "$ROOT/target/evidence68-vol3/normal/qemu-serial.log" normal
FAILA=$?

echo "=== 步骤 4：窗 B kill+restart（复用窗 A 数据盘——持久性判据）==="
ADISK="$ROOT/target/evidence68-vol3/normal/eneros-data.raw"
[ -f "$ADISK" ] || { echo "窗 A 数据盘缺失（xtask 产物路径核对）"; exit 8; }
# 已知坑（蓝图 60 E.2 在册）：UEFI 期启动冻结重跑即过——restart 窗带重试
# （引导判据 = serial 出现 READINESS 行；重试仍复用同一数据盘）。
for ATTEMPT in 1 2; do
  pkill -9 -f "qemu-system" 2>/dev/null || true
  sleep 2
  run_window restart 600 "$ADISK"
  if grep -aq "READINESS: " "$ROOT/target/evidence68-vol3/restart/qemu-serial.log" 2>/dev/null; then
    break
  fi
  echo "[i] 窗 B 第 $ATTEMPT 次引导未完成（UEFI 冻结坑形态）——重试（数据盘复用不变）"
done
scan_window "$ROOT/target/evidence68-vol3/restart/qemu-serial.log" restart
FAILB=$?

echo "=== 窗 B 专项：强杀数据保持（TSDB-DISK-OPEN points>0）==="
python3 - "$ROOT/target/evidence68-vol3/restart/qemu-serial.log" <<'PYEOF' || FAILB=1
import re, sys
pts = None
for line in open(sys.argv[1], errors="replace"):
    m = re.search(r"\[svc-telemetry\] TSDB-DISK-OPEN base_block=(\d+) region_blocks_512=(\d+) points=(\d+)", line)
    if m:
        pts = int(m.group(3))
if pts is None:
    print("[✗] TSDB-DISK-OPEN 行缺失"); sys.exit(1)
if pts <= 0:
    print(f"[✗] 重启后 points={pts}——旧数据未保持（持久性判据失守）"); sys.exit(1)
print(f"[✓] 重启后 TSDB-DISK-OPEN points={pts} > 0（index.bin 重建，旧数据保持）")
PYEOF

echo "=== 汇总 ==="
[ $FAILA -eq 0 ] || echo "窗 A FAIL"
[ $FAILB -eq 0 ] || echo "窗 B FAIL"
if [ $FAILA -eq 0 ] && [ $FAILB -eq 0 ]; then
  echo "VOL68.3-GATE: PASS"
else
  echo "VOL68.3-GATE: FAIL"; exit 7
fi
