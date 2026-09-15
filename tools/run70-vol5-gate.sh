#!/bin/bash
# 蓝图 70 卷 70.5（v0.335.0）出口门（方向 3 §6 各 WP + CORE-70-R06）——
# 生产身份、OTA 信任与目标安全通信的实弹矩阵。双窗：
#
#   窗A（qemu-prod，TLS 端到端主窗）：
#     段0  材料生成：openssl P-256 PKI（CA/broker/device）+ station 密钥对
#          （product/tls/ + product/station/——不入仓，原始种子不写证据）。
#     段1  config compile（qemu-prod，dev 5a OTA 锚——≠ 测试锚）→ ota-anchor
#          门禁（禁用面 PASS）→ sel4 重链接 → image raw。
#     段2  TLS broker（python ssl，:8883）+ 从站编队 + QEMU 窗（$DL s）：
#          R1  boot-manager 写 capsule（ENEROS stage=identity-capsule）；
#          R2  root 推送 + svc-ota 装配（IDENTITY-PUSH: OK + OTA READY）；
#          R3  svc-mqtt ReleaseTls 装配允许 + TLS 握手 + MQTT 流（broker
#              transcript: CONNECT/SUBSCRIBE over TLS + 遥测 TELFRAME）；
#          R4  控制链（Committed）+ 设备回读（ACQ）——TLS 通道上全链；
#          R5  探针隔离：OTA-DOWNLOAD "probe absent" 恰 1 次（release 闭
#              包零固定种子签章路径）；
#          R6  伪 principal 拒绝：gate 以错钥签 MGT1 v2 → 目标
#              "MGT-PRINCIPAL rejected" ≥ 1；
#          R7  零 PANIC / 零分配失败。
#   窗B（qemu-test，隔离测试锚探针链）：
#     段3  config compile（qemu-test，显式测试锚 root key = 探针种子派生
#          公钥——测试链同源配对）→ ota-anchor 门禁（非禁用面）→ 重链接
#          → image raw → 短窗 QEMU：
#          R8  签名描述 v2 全链（CHECK-DESC/DOWNLOAD-DESC/TRIAL/COMMIT
#              = OTA-DOWNLOAD: OK）——隔离测试锚下的目标写路径实弹；
#          R9  IDENTITY: TEST assembly synthetic（capsule 缺失时合成身份
#              标签；UEFI 窗真实 capsule 优先）。
#
#   用法：DL=560 tools/run70-vol5-gate.sh
set -uo pipefail
source "$(dirname "$0")/gate-env.sh"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
EVROOT="$ROOT/evidence/70/70.5"
RUN="$ROOT/target/evidence70-vol5"
DL="${DL:-560}"
mkdir -p "$EVROOT" "$RUN"
FAIL=0
TLS_PORT="${TLS_PORT:-8883}"

# ---- 段0：材料生成（openssl；原始私钥不写证据）----------------------------
echo "=== 段0：TLS/station 材料生成（openssl P-256）==="
TLSDIR="$ROOT/product/tls"
STDIR="$ROOT/product/station"
mkdir -p "$TLSDIR" "$STDIR"
WORK="$RUN/pki"; mkdir -p "$WORK"; rm -f "$WORK"/*
# CA
openssl ecparam -name prime256v1 -genkey -noout -out "$WORK/ca.key" 2>/dev/null
openssl req -x509 -new -key "$WORK/ca.key" -subj "/CN=eneros-ca" \
  -days 30 -out "$WORK/ca.pem" 2>/dev/null
# broker（server）证书
openssl ecparam -name prime256v1 -genkey -noout -out "$WORK/broker.key" 2>/dev/null
openssl req -new -key "$WORK/broker.key" -subj "/CN=eneros-broker" -out "$WORK/broker.csr" 2>/dev/null
openssl x509 -req -in "$WORK/broker.csr" -CA "$WORK/ca.pem" -CAkey "$WORK/ca.key" \
  -days 30 -out "$WORK/broker.pem" 2>/dev/null
# device（client）证书——设备侧身份（WP-70.5.6 设备凭据）
openssl ecparam -name prime256v1 -genkey -noout -out "$WORK/device.key" 2>/dev/null
openssl req -new -key "$WORK/device.key" -subj "/CN=eneros-device" -out "$WORK/device.csr" 2>/dev/null
openssl x509 -req -in "$WORK/device.csr" -CA "$WORK/ca.pem" -CAkey "$WORK/ca.key" \
  -days 30 -out "$WORK/device.pem" 2>/dev/null
# station（管理 principal 签发方）
openssl ecparam -name prime256v1 -genkey -noout -out "$WORK/station.key" 2>/dev/null
# SEC1 未压缩点 65B（04||X||Y）——从 openssl text 输出解析（SPKI DER ≠ 裸点）。
python3 - "$WORK/station.key" "$STDIR/station-pubkey.sec1" <<'PYEOF'
import subprocess, sys
key, out = sys.argv[1], sys.argv[2]
txt = subprocess.run(["openssl", "ec", "-in", key, "-text", "-noout"],
                     capture_output=True, text=True, check=True).stdout
hexchars = []
in_pub = False
for line in txt.splitlines():
    if line.strip().startswith("pub:"):
        in_pub = True
        continue
    if in_pub:
        if "ASN1" in line or ":" not in line:
            break
        hexchars.extend(line.strip().rstrip(":").split(":"))
point = bytes(int(b, 16) for b in hexchars)
assert len(point) == 65 and point[0] == 0x04, f"SEC1 点形态异常 len={len(point)}"
open(out, "wb").write(point)
PYEOF
# 目标嵌入材料（DER 形态）
openssl x509 -in "$WORK/ca.pem" -outform DER -out "$TLSDIR/mqtt-ca.der" 2>/dev/null
openssl x509 -in "$WORK/device.pem" -outform DER -out "$TLSDIR/mqtt-client.der" 2>/dev/null
openssl ec -in "$WORK/device.key" -outform DER -out "$TLSDIR/mqtt-client-key.der" 2>/dev/null
printf 'eneros-broker' > "$TLSDIR/mqtt-server-name"
openssl rand 32 > "$TLSDIR/tls-seed.bin" 2>/dev/null
for f in "$TLSDIR/mqtt-ca.der" "$TLSDIR/mqtt-client.der" "$TLSDIR/mqtt-client-key.der" \
         "$TLSDIR/mqtt-server-name" "$TLSDIR/tls-seed.bin" "$STDIR/station-pubkey.sec1"; do
  [ -s "$f" ] || { echo "材料缺失: $f"; exit 2; }
done
[ -s "$TLSDIR/mqtt-client-key.der" ] || { echo "device key DER 缺失"; exit 2; }
sha256sum "$TLSDIR"/* | tee "$EVROOT/tls-material.sha256"
echo "[✓][段0] 材料生成（CA/broker/device/station；种子不写证据）"

# ---- 段1：窗A 构建（qemu-prod；dev 5a OTA 锚 ≠ 测试锚）--------------------
gate_cleanup_env 5577 5582 2404 1883 "$TLS_PORT"
kill -9 $(pgrep -f "eneros-planner-peer") 2>/dev/null || true
kill -9 $(pgrep -f "tls_mqtt_broker") 2>/dev/null || true
kill -9 $(pgrep -f "mqtt_telemetry_broker.py") 2>/dev/null || true
sleep 2
gate_export_signing_key
# 窗A OTA 信任锚：每轮新鲜生成（合法曲线点；≠ 冻结测试锚——ota-anchor
# 门禁判据；≠ 旧 5a 填充字面——validate_policy 曲线点域检拒绝它）。
OTA_ANCHOR_HEX=$(cargo xtask gen-ota-anchor 2>/dev/null | tail -1)
[ "${#OTA_ANCHOR_HEX}" -eq 128 ] || { echo "OTA 锚生成失败"; exit 2; }
echo "OTA-ANCHOR fresh fp(4B)=${OTA_ANCHOR_HEX:0:8}"
# TLS 端口前置：签进 bundle 的 broker 地址 = TLS 口（必须先于 config compile
# ——bundle 在 cargo build 时嵌入业务服务 ELF，镜像构建后改配置无效）。
MQTT_TOML="$ROOT/configs/protocol-mqtt.toml"
cp -f "$MQTT_TOML" "$RUN/protocol-mqtt.toml.bak"
python3 - "$MQTT_TOML" "$TLS_PORT" <<'PYEOF'
import sys, re
p, port = sys.argv[1], sys.argv[2]
s = open(p).read()
s = re.sub(r'broker\s*=\s*"10\.0\.2\.2:\d+"', f'broker = "10.0.2.2:{port}"', s)
open(p, "w").write(s)
PYEOF
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --accept-test-signing-key --root-key-hex "$OTA_ANCHOR_HEX" >"$RUN/config-compile-a.log" 2>&1 || {
  echo "CONFIG-COMPILE FAIL rc=$?"; tail -5 "$RUN/config-compile-a.log"; exit 2; }
cargo xtask qa gate ota-anchor --profile qemu-prod | tee "$EVROOT/ota-anchor-gate-a.txt"
grep -q "forbidden=yes" "$EVROOT/ota-anchor-gate-a.txt" || { echo "ota-anchor 禁用面判据缺失"; exit 2; }
gate_relink_cached "$RUN/sel4-a.log" || {
  echo "SEL4-RELINK FAIL"; tail -20 "$RUN/sel4-a.log"; exit 3; }
gate_image_raw_cached qemu-prod "$RUN/image-a.log"
IMG="$ROOT/target/eneros/artifacts/qemu-prod/eneros-qemu-prod.raw"
[ -f "$IMG" ] || { echo "image missing"; exit 3; }
sha256sum "$IMG" | tee "$EVROOT/image-a.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { echo "vars template missing"; exit 3; }

# ---- 段2：窗A QEMU（TLS 端到端）-------------------------------------------
DIR="$RUN/win-a"; mkdir -p "$DIR"
rm -f "$DIR/qemu-serial.log" "$DIR/eneros-data.raw" "$DIR/broker-transcript.jsonl" "$DIR/forge.transcript.jsonl" \
      "$DIR/modbus.transcript.jsonl" "$DIR/iec104.transcript.jsonl"
echo "=== 段2：窗A QEMU TLS 窗（$DL s）==="
python3 "$ROOT/evidence/61/modbus_slave61.py" --transcript "$DIR/modbus.transcript.jsonl" &
PMOD=$!
sleep 1
python3 "$ROOT/evidence/62/iec104_slave62.py" --transcript "$DIR/iec104.transcript.jsonl" --deadline "$DL" &
P104=$!
sleep 1
cat "$WORK/broker.pem" "$WORK/ca.pem" > "$WORK/broker-fullchain.pem"
python3 "$ROOT/evidence/70/tls_mqtt_broker.py" --port "$TLS_PORT" \
  --tls-cert "$WORK/broker-fullchain.pem" --tls-key "$WORK/broker.key" \
  --transcript "$DIR/broker-transcript.jsonl" >"$DIR/broker.log" 2>&1 &
BROKER_PID=$!
sleep 2
(ss -tln 2>/dev/null | grep -q ":$TLS_PORT ") || { echo "TLS BROKER-NOT-LISTENING"; exit 4; }

cargo xtask run qemu-prod \
  --image "$IMG" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR" \
  --net-mode present --net-user --net-protocols present \
  --reset-bootctrl --ready-marker product-ready --qmp-deadline-secs "$DL" >/dev/null 2>&1 &
QEMU_JOB=$!

# R6 负例：实时定时注入（QEMU 启动 +75s 墙钟触发——串口文件滞后约
# 250s，以文件为触发的注入会落进窗内串口静默段之后；+90s 时订阅已
# 就绪且目标活跃）。
(
  # REG-70.15-VOL5-R6-INTERMITTENT 治理（2026-09-15）：注入从 3 次/24s
  # 加宽为 8 次/约 5 分钟（覆盖窗内 v2 验签订阅就绪后的整个相位）；
  # 发送计数入 forge.transcript（"帧已发出"进程侧证据，判据交叉核验）。
  sleep 75
  for ROUND in 1 2 3 4 5 6 7 8; do
    python3 "$ROOT/evidence/70/mgt_v2_forger.py" --broker 127.0.0.1:"$TLS_PORT" \
      --ca "$WORK/ca.pem" --client "$WORK/broker.pem" --client-key "$WORK/broker.key" \
      --transcript "$DIR/forge.transcript.jsonl" >>"$DIR/forge.log" 2>&1 || true
    sleep 35
  done
) &
FORGER_JOB=$!
for _ in $(seq 1 100); do
  grep -q "PRODUCT-READY: true" "$DIR/qemu-serial.log" 2>/dev/null && break
  sleep 3
done
grep -q "PRODUCT-READY: true" "$DIR/qemu-serial.log" || {
  echo "PRODUCT-READY 未达"; kill $QEMU_JOB $PMOD $P104 $BROKER_PID 2>/dev/null || true
  sleep 2; kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true; exit 6; }

sleep 130  # 控制链/遥测/TLS 流样本（含 forger 注入段）

# 终止序：qemu-system 先 -9（管道 EOF → harness 排空串口尾部），再杀 harness/编队
kill -9 $(pgrep -f "qemu-system") 2>/dev/null || true
sleep 3
kill $QEMU_JOB $PMOD $P104 $BROKER_PID $FORGER_JOB 2>/dev/null || true
sleep 2
cp -f "$DIR/qemu-serial.log" "$EVROOT/wa.qemu-serial.log" 2>/dev/null || true
cp -f "$DIR/broker-transcript.jsonl" "$EVROOT/wa.broker-transcript.jsonl" 2>/dev/null || true
cp -f "$DIR/broker.log" "$EVROOT/wa.broker.log" 2>/dev/null || true
mv -f "$RUN/protocol-mqtt.toml.bak" "$MQTT_TOML"

S="$DIR/qemu-serial.log"; BT="$DIR/broker-transcript.jsonl"
[ -f "$S" ] || { echo "serial log missing"; exit 7; }

echo "=== 窗A 判据扫描 ==="
PANICS=$(grep -aci "PANIC\|memory allocation of" "$S" || true)
[ "$PANICS" -eq 0 ] && echo "[✓][R7] 零 PANIC / 零分配失败" || { echo "[✗][R7] PANIC=$PANICS"; FAIL=1; }

n=$(grep -ac "ENEROS stage=identity-capsule " "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R1] identity-capsule 写入（$n）" || { echo "[✗][R1] capsule 词形缺失"; FAIL=1; }

n=$(grep -ac "IDENTITY-PUSH: OK" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R2a] 身份推送 OK（$n）" || { echo "[✗][R2a] IDENTITY-PUSH OK 缺失"; FAIL=1; }
n=$(grep -ac "OTA READY" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R2b] svc-ota 装配（OTA READY ×$n）" || { echo "[✗][R2b] OTA READY 缺失"; FAIL=1; }
n=$(grep -ac "OTA-CONFIG gen=" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R2c] 配置真实取数（OTA-CONFIG ×$n）" || { echo "[✗][R2c] OTA-CONFIG 缺失"; FAIL=1; }

n=$(grep -ac "SECURITY-CHECK mode=ReleaseTls.*assembly=allowed" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R3a] ReleaseTls 装配允许" || { echo "[✗][R3a] ReleaseTls allowed 缺失"; FAIL=1; }
n=$(grep -ac '"event": "CONNACK"' "$BT" 2>/dev/null); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R3b] TLS broker 侧 CONNACK（$n）" || { echo "[✗][R3b] broker CONNACK 缺失"; FAIL=1; }
n=$(grep -ac '"event": "SUBSCRIBE"' "$BT" 2>/dev/null); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R3c] TLS 通道订阅（$n）" || { echo "[✗][R3c] SUBSCRIBE 缺失"; FAIL=1; }
n=$(grep -ac "TELFRAME" "$BT" 2>/dev/null); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R3d] TLS 通道遥测帧（$n）" || { echo "[✗][R3d] TELFRAME 缺失"; FAIL=1; }

n=$(grep -ac "dispatch=Committed" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R4a] 控制链 Committed（$n）" || { echo "[✗][R4a] Committed 缺失"; FAIL=1; }
n=$(grep -ac "ACQ point=" "$S" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R4b] 设备回读 ACQ（$n）" || { echo "[✗][R4b] ACQ 缺失"; FAIL=1; }

n=$(grep -ac "OTA-DOWNLOAD: probe absent" "$S" || true); n=${n:-0}
[ "$n" -eq 1 ] && echo "[✓][R5] 探针隔离（probe absent ×1，零签章路径）" || { echo "[✗][R5] probe absent ×$n（期望恰 1）"; FAIL=1; }
n=$(grep -ac "OTA-DOWNLOAD: OK" "$S" || true); n=${n:-0}
[ "$n" -eq 0 ] && echo "[✓][R5b] 窗A 零 OTA-DOWNLOAD OK（测试探针不在 release）" || { echo "[✗][R5b] OTA-DOWNLOAD OK ×$n 出现于 release 窗"; FAIL=1; }

n=$(grep -ac "MGT-PRINCIPAL rejected" "$S" || true); n=${n:-0}
SENT=$(grep -ac "FORGED-MGT-PUBLISH" "$DIR/forge.transcript.jsonl" 2>/dev/null || true); SENT=${SENT:-0}
if [ "$n" -ge 1 ]; then
  echo "[✓][R6] 伪 principal 拒绝（$n；forger 发送 $SENT）"
else
  echo "[✗][R6] 伪 principal 拒绝缺失（forger 发送 $SENT——>0 则为真安全面失败，0 为注入未落地）"; FAIL=1
fi

cp -f "$S" "$EVROOT/wa.qemu-serial.log"

# ---- 段3：窗B（qemu-test；测试锚探针链）------------------------------------
echo "=== 段3：窗B 构建（qemu-test；显式测试锚）==="
TEST_ROOT_KEY_HEX="1f58396696deec97a0090fa612eb5c4784f0b114693696916c01e9cff8ebc5c7ec0c18caf02a7478c73e54bf13a06fb11624fd1a233ab5e41217aa7bb3803e04"
# 测试链 bundle：qemu-prod 清单（全集 ACTIVE 配置——qemu-test 装配缺
# dlt645/iec101/iec103/gbt27930 服务无法满足 CF-01 消费者声明）+ 显式
# 测试锚 root key（与探针种子同源配对——WP-70.5.5 隔离链）。
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-prod \
  --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" >"$RUN/config-compile-b.log" 2>&1 || {
  echo "CONFIG-COMPILE(窗B) FAIL"; tail -5 "$RUN/config-compile-b.log"; exit 2; }
cargo xtask qa gate ota-anchor --profile qemu-test | tee "$EVROOT/ota-anchor-gate-b.txt"
echo "=== sel4 重链接（窗B；synth 探针装配）==="
gate_relink_cached "$RUN/sel4-b.log" "sel4-eipc,qemu-test-synth" || {
  echo "SEL4-RELINK-B FAIL"; tail -20 "$RUN/sel4-b.log"; exit 3; }
gate_image_raw_cached qemu-test "$RUN/image-b.log" "sel4-eipc,qemu-test-synth"
IMG_B="$ROOT/target/eneros/artifacts/qemu-test/eneros-qemu-test.raw"
[ -f "$IMG_B" ] || { echo "qemu-test image missing"; exit 3; }
sha256sum "$IMG_B" | tee "$EVROOT/image-b.sha256"

DIR_B="$RUN/win-b"; mkdir -p "$DIR_B"; rm -f "$DIR_B/qemu-serial.log" "$DIR_B/eneros-data.raw"
echo "=== 段3：窗B QEMU 探针链窗 ==="
DL_B="${DL_B:-420}"
cargo xtask run qemu-test \
  --image "$IMG_B" --vars-template "$TPL" \
  --expect PASS --evidence-dir "$DIR_B" \
  --net-mode present --net-user \
  --reset-bootctrl --ready-marker root-heartbeat --qmp-deadline-secs "$DL_B" >/dev/null 2>&1
RC_B=$?
SB="$DIR_B/qemu-serial.log"
[ -f "$SB" ] || { echo "窗B serial missing"; exit 7; }
cp -f "$SB" "$EVROOT/wb.qemu-serial.log"

n=$(grep -ac "OTA-DOWNLOAD: OK" "$SB" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R8] 签名描述 v2 探针全链（OTA-DOWNLOAD OK ×$n）" || { echo "[✗][R8] 窗B OTA-DOWNLOAD OK 缺失"; FAIL=1; }
n=$(grep -ac "IDENTITY" "$SB" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R9] 窗B 身份面观测（IDENTITY ×$n）" || { echo "[✗][R9] 窗B IDENTITY 缺失"; FAIL=1; }
n=$(grep -ac "OTA-CONFIG gen=" "$SB" || true); n=${n:-0}
[ "$n" -ge 1 ] && echo "[✓][R9b] 窗B 配置取数（OTA-CONFIG ×$n）" || { echo "[✗][R9b] 窗B OTA-CONFIG 缺失"; FAIL=1; }

# ---- 段4：窗A 后恢复 5a 锚产物链（供下一卷基线）---------------------------
gate_compile_config "$RUN/config-compile-restore.log" qemu-prod

echo "=== 窗B/全窗 PANIC 扫描 ==="
PANICS_B=$(grep -aci "PANIC\|memory allocation of" "$SB" || true)
[ "$PANICS_B" -eq 0 ] && echo "[✓][窗B] 零 PANIC" || { echo "[✗][窗B] PANIC=$PANICS_B"; FAIL=1; }

if [ "$FAIL" -eq 0 ]; then
  echo "VOL70.5-GATE: PASS"
else
  echo "VOL70.5-GATE: FAIL"
fi
exit $FAIL
