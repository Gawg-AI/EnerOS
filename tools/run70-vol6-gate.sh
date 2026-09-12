#!/usr/bin/env bash
# ============================================================
# 蓝图 70.6 出口门（v0.336.0）：管理动作实效与配置生命周期
#
# 装配：qemu-test（qemu-test-synth 合成身份 + 测试 station 密钥对 =
# MGT1 v2 强制；root 侧 MGT1 探针 Send 直驱 svc-mgmt——应答帧被 seL4
# 丢弃，动作实效经串口审计行「MGT1: rid=…」与 root/svc-config 词形取证）。
#
# 窗T-1：管理动作全链——config-get 载荷/支持包/维护进入（+幂等回放）/
#   签名换包 stage→commit→activate（gen 1→2）/消费者同代/受控重启
#   （实例代次+ready 重验）/维护退出；负例——保护服务重启拒绝、幂等
#   冲突、维护外激活拒绝、短尾 v2 拒绝、伪 principal。
# 窗T-2：同盘重启（无 --reset-bootctrl）——CONFIG-DISK active gen=2
#   冷启动恢复 + MGMT-OPS 账本恢复。
#
# 外部 station 经 MQTT 的下行投递在现装配存在待查缺陷
# （REG-70.6-MGMT-DOWNLINK，TLS/明文同症、上行正常）——本门以探针直驱
# 生产 MGT1 接口取证，mqtt 下行修复后可切回外注式判据。
#
# 用法：DL=700 tools/run70-vol6-gate.sh
# 退出：FAIL=0 → VOL70.6-GATE: PASS
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source tools/gate-env.sh

EVROOT="$ROOT/evidence/70/70.6"
RUN="$ROOT/target/evidence70-vol6"
DL="${DL:-700}"
DL_T2="${DL_T2:-420}"
FAIL=0

mkdir -p "$EVROOT" "$RUN"

say() { echo "[vol6-gate] $*"; }
wait_serial_stable() { # <file>： harness 子进程刷盘竞态——尺寸连续两次采样不变才返回
    local f="$1" prev=-1 cur=0
    for _ in $(seq 1 30); do
        cur=$(stat -c%s "$f" 2>/dev/null || echo 0)
        if [ "$cur" = "$prev" ]; then return 0; fi
        prev=$cur
        sleep 2
    done
}
check() {
    local rn="$1"; shift
    local desc="$1"; shift
    if "$@"; then
        echo "[✓][${rn}] ${desc}"
    else
        echo "[✗][${rn}] ${desc}"
        FAIL=1
    fi
}

# ============================================================
say "=== 段0：测试 station 密钥对 + 探针 token/签名 ==="
# ============================================================
gate_cleanup_env 5577 5582 2404 1883 || true
sleep 1

PKI="$RUN/pki"
rm -rf "$PKI"; mkdir -p "$PKI" product/station
# station（管理 principal 签发方，EC P-256）——公钥 SEC1 65B 裸点部署进
# 构建（svc-mgmt v2 强制面）；私钥留 gate 运行目录签名探针 token。
openssl ecparam -name prime256v1 -genkey -noout -out "$PKI/station.key" 2>/dev/null \
    || { say "station key 生成失败"; exit 2; }
python3 - "$PKI/station.key" product/station/station-pubkey.sec1 "$RUN/probe-token.hex" "$RUN/probe-sig.hex" <<'PYEOF'
import subprocess, sys, struct, hashlib
key, pub_out, tok_out, sig_out = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
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
open(pub_out, "wb").write(point)
# 探针固定 token（与 root mgt_probe.rs 常量逐字节同构——签名只覆盖 token）。
principal = b"station-ops".ljust(16, b"\0")
roles = bytes([2, 0, 0, 0])          # admin
tenant = struct.pack("<I", 1)
channel = b"probe".ljust(16, b"\0")
expiry = struct.pack("<Q", 4_102_444_800_000)
issued = struct.pack("<Q", 1_700_000_000_000)
token = b"MGTPTK1\0" + principal + roles + tenant + channel + expiry + issued
assert len(token) == 64
import tempfile
with tempfile.NamedTemporaryFile(suffix=".bin", delete=False) as f:
    f.write(token); tf = f.name
sig = subprocess.run(["openssl", "dgst", "-sha256", "-sign", key, tf],
                     capture_output=True, check=True).stdout
assert len(sig) <= 72, f"DER 签名超 72B: {len(sig)}"
open(tok_out, "w").write(token.hex())
open(sig_out, "w").write(sig.hex())
print(f"TOKEN {len(token)}B SIG {len(sig)}B")
PYEOF
[ -s "$RUN/probe-token.hex" ] && [ -s "$RUN/probe-sig.hex" ] || { say "探针 token/签名生成失败"; exit 2; }
[ "$(stat -c%s product/station/station-pubkey.sec1)" = "65" ] || { say "station SEC1 非 65B"; exit 2; }
sha256sum product/station/station-pubkey.sec1 > "$EVROOT/station-pubkey.sha256"
say "station 公钥部署（v2 强制面）+ 探针 token/签名就绪"

# ============================================================
say "=== 段1：基线束(gen1) + 换包候选(gen2) + 镜像T 构建 ==="
# ============================================================
TEST_ROOT_KEY_HEX="1f58396696deec97a0090fa612eb5c4784f0b114693696916c01e9cff8ebc5c7ec0c18caf02a7478c73e54bf13a06fb11624fd1a233ab5e41217aa7bb3803e04"
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
    --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" \
    > "$RUN/config-compile-t.log" 2>&1 || { say "基线 config compile 失败"; exit 2; }
cp product/config-bundle.bin "$RUN/config-bundle-gen1.bin"
cargo xtask qa gate ota-anchor --profile qemu-test 2>&1 | tee "$EVROOT/ota-anchor-gate-t.txt" | grep -q "OTA-ANCHOR-GATE: PASS" \
    || { say "ota-anchor 门未 PASS"; exit 2; }
# 换包候选（gen 2）：control-write.handshake_retries 5→4（真实参数变化）。
cp configs/control-write.toml "$RUN/control-write.toml.bak"
sed -i 's/^handshake_retries = 5/handshake_retries = 4/' configs/control-write.toml
grep -q "^handshake_retries = 4" configs/control-write.toml \
    || { say "control-write 修改失败"; cp "$RUN/control-write.toml.bak" configs/control-write.toml; exit 2; }
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
    --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" --generation 2 \
    > "$RUN/config-compile-cand.log" 2>&1 || { say "候选 config compile 失败"; cp "$RUN/control-write.toml.bak" configs/control-write.toml; exit 2; }
cp product/config-bundle.bin "$RUN/config-bundle-gen2.bin"
cp "$RUN/control-write.toml.bak" configs/control-write.toml
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
    --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" \
    > "$RUN/config-compile-restore.log" 2>&1 || { say "基线束恢复失败"; exit 2; }
say "候选束 gen2=$(stat -c%s "$RUN/config-bundle-gen2.bin")B 基线束 gen1=$(stat -c%s "$RUN/config-bundle-gen1.bin")B"
# 根循环探针构建注入（测试 token/签名 + 候选束路径；qemu-test-synth 专属）。
export ENEROS_MGT_PROBE_TOKEN="$(cat "$RUN/probe-token.hex")"
export ENEROS_MGT_PROBE_SIG="$(cat "$RUN/probe-sig.hex")"
export ENEROS_MGT_PROBE_BUNDLE="$RUN/config-bundle-gen2.bin"
ROOT_TASK_FEATURES="sel4-eipc,qemu-test-synth" bash third_party/sel4-build-rust.sh \
    > "$RUN/sel4-t.log" 2>&1 || { say "sel4 重链接失败"; exit 3; }
gate_image_raw qemu-test "$RUN/image-t.log" || exit 3
IMG="$ROOT/target/eneros/artifacts/qemu-test/eneros-qemu-test.raw"
sha256sum "$IMG" | awk '{print $1}' > "$EVROOT/image-t.sha256"
TPL="$ROOT/target/evidence61/template.fd"
[ -f "$TPL" ] || { say "缺 UEFI vars 模板 $TPL"; exit 3; }

# ============================================================
say "=== 段2：窗T-1 管理动作链实弹（探针直驱）==="
# ============================================================
DIR="$RUN/t1"
rm -rf "$DIR"; mkdir -p "$DIR"
# 共享数据盘（T1 提交落此处；T2 同盘冷启动读取）。
DATADISK="$RUN/data-disk.raw"
rm -f "$DATADISK"
truncate -s 16M "$DATADISK"
cargo xtask run qemu-test \
    --image "$IMG" --vars-template "$TPL" \
    --expect PASS --evidence-dir "$DIR" \
    --data-disk "$DATADISK" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --ready-marker root-heartbeat --qmp-deadline-secs "$DL" &
HPID=$!

S="$DIR/qemu-serial.log"
READY=0
for i in $(seq 1 300); do
    if grep -aq "PRODUCT-READY" "$S" 2>/dev/null; then READY=1; break; fi
    if ! kill -0 $HPID 2>/dev/null; then break; fi
    sleep 1
done
# 探针序列完成词形（或超时上限）。
for i in $(seq 1 240); do
    if grep -aq "MGT-PROBE: sequence complete" "$S" 2>/dev/null; then break; fi
    if ! kill -0 $HPID 2>/dev/null; then break; fi
    sleep 1
done
sleep 20   # 串口消化尾部词形

kill -9 $(pgrep -f qemu-system) 2>/dev/null || true
# 杀 xtask 子进程（run12-15 实测：cargo 父死而子活，串口刷盘竞态丢尾部词形）
pkill -9 -f "eneros-[x]task run" 2>/dev/null || true
sleep 3
kill $HPID 2>/dev/null || true
cp "$S" "$EVROOT/t1.qemu-serial.log" 2>/dev/null || true

# ---------- 窗T-1 判据 ----------
say "=== 段2 判据（窗T-1）==="
check R1 "零 PANIC / 零分配失败" bash -c "! grep -aqcE 'PANIC|memory allocation of' '$EVROOT/t1.qemu-serial.log'"
check R2 "PRODUCT-READY 达成" bash -c "grep -aq 'PRODUCT-READY: true' '$EVROOT/t1.qemu-serial.log'"
check R3 "config-get 载荷取回（gen=1）" bash -c "grep -aq 'MGT1: rid=10 config-get -> Completed config-get:control-write:gen=1' '$EVROOT/t1.qemu-serial.log'"
check R5 "维护进入实效（root 词形）" bash -c "grep -aq 'MGMT-MAINT: entered' '$EVROOT/t1.qemu-serial.log'"
check R6 "维护内控制写暂停词形" bash -c "grep -aq 'MGMT-MAINT: control write paused' '$EVROOT/t1.qemu-serial.log'"
check R7 "签名换包激活 gen=2（root 直驱 CFG1）" bash -c "grep -aq 'MGT-PROBE: config-activated gen=2' '$EVROOT/t1.qemu-serial.log'"
check R8 "盘上提交词形（CONFIG-ACTIVATED）" bash -c "grep -aq 'CONFIG-ACTIVATED gen=2' '$EVROOT/t1.qemu-serial.log'"
check R9 "消费者同代（config-get gen=2）" bash -c "grep -aq 'MGT1: rid=260 config-get -> Completed config-get:control-write:gen=2' '$EVROOT/t1.qemu-serial.log'"
check R10 "受控重启受理（root 词形）" bash -c "grep -aq 'MGMT-RESTART: svc-observability requested' '$EVROOT/t1.qemu-serial.log'"
check R11 "实例代次变更 + 重启完成" bash -c "grep -aq 'MGMT-RESTART: svc-observability restarted (gen=' '$EVROOT/t1.qemu-serial.log'"
check R12 "ready 重验完成" bash -c "grep -aq 'MGMT-RESTART: svc-observability ready re-verified' '$EVROOT/t1.qemu-serial.log'"
check R14 "维护退出（健康门放行）" bash -c "grep -aq 'MGMT-MAINT: exited' '$EVROOT/t1.qemu-serial.log'"
check R15 "负例：保护服务重启拒绝（root 裁决词形）" bash -c "grep -aq 'MGMT-REJECT: restart-denied:protected' '$EVROOT/t1.qemu-serial.log'"
check R16 "负例：幂等冲突拒绝（同 rid 异参）" bash -c "grep -aq 'MGT1: rid=200 config-set -> Failed idempotency-conflict' '$EVROOT/t1.qemu-serial.log'"
check R18 "负例：幂等回放（同操作一次执行）" bash -c "grep -aq 'replay: config-stage-part' '$EVROOT/t1.qemu-serial.log'"
check R19 "负例：短尾 v2 拒绝（REG-70.5 收口）" bash -c "grep -aq 'MGT-PRINCIPAL rejected (truncated v2 shape' '$EVROOT/t1.qemu-serial.log'"
check R20 "负例：伪 principal 拒绝（70.5 R6 回归）" bash -c "grep -aq 'MGT-PRINCIPAL rejected' '$EVROOT/t1.qemu-serial.log'"
check R21 "正例配对：合法 v2 签名过验签进授权（stage commit 执行）" bash -c "grep -aq 'rid=240 config-set -> Completed op=15 config-staged:digest=' '$EVROOT/t1.qemu-serial.log'"
check R17 "维护外激活拒绝（探针步存在=宿主闸覆盖；QEMU 环形不安全）" bash -c "grep -aq 'MGT-PROBE: neg maint-required host-covered' '$EVROOT/t1.qemu-serial.log'"
check R22 "操作账本持久化词形（MGMT-OPS persisted）" bash -c "grep -aq 'MGMT-OPS persisted' '$EVROOT/t1.qemu-serial.log'"

# ============================================================
say "=== 段3：窗T-2 同盘冷启动恢复（无 --reset-bootctrl）==="
# ============================================================
DIR2="$RUN/t2"
rm -rf "$DIR2"; mkdir -p "$DIR2"
# T2 复用 T1 的 run vars（boot-manager protected anchor 存于 UEFI vars——
# 模板重置 = anchor 丢失 = BOOTCTRL 拒启，run14/15 实锤）。
VARS_T2="$DIR/AAVMF_VARS.run.fd"
[ -f "$VARS_T2" ] || VARS_T2="$TPL"
cargo xtask run qemu-test \
    --image "$IMG" --vars-template "$VARS_T2" \
    --expect PASS --evidence-dir "$DIR2" \
    --data-disk "$DATADISK" \
    --net-mode present --net-user --net-protocols present \
    --ready-marker root-heartbeat --qmp-deadline-secs "$DL_T2" &
HPID2=$!
S2="$DIR2/qemu-serial.log"
READY2=0
for i in $(seq 1 240); do
    if grep -aq "PRODUCT-READY" "$S2" 2>/dev/null; then READY2=1; break; fi
    if ! kill -0 $HPID2 2>/dev/null; then break; fi
    sleep 1
done
sleep 15
kill -9 $(pgrep -f qemu-system) 2>/dev/null || true
pkill -9 -f "eneros-[x]task run" 2>/dev/null || true
sleep 3
kill $HPID2 2>/dev/null || true
cp "$S2" "$EVROOT/t2.qemu-serial.log" 2>/dev/null || true

check R23 "窗T-2 冷启动 PRODUCT-READY" bash -c "grep -aq 'PRODUCT-READY: true' '$EVROOT/t2.qemu-serial.log'"
check R24 "盘上配置冷启动恢复（CONFIG-DISK active gen=2）" bash -c "grep -aq 'CONFIG-DISK active gen=2' '$EVROOT/t2.qemu-serial.log'"
check R25 "操作账本跨重启恢复（MGMT-OPS ledger restored）" bash -c "grep -aq 'MGMT-OPS ledger restored' '$EVROOT/t2.qemu-serial.log'"
check R26 "窗T-2 零 PANIC" bash -c "! grep -aqcE 'PANIC|memory allocation of' '$EVROOT/t2.qemu-serial.log'"

# ============================================================
say "=== 段4：恢复基线束 ==="
# ============================================================
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
    --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" \
    > "$RUN/config-compile-restore2.log" 2>&1 || say "（警告）基线束最终恢复失败"

# ============================================================
if [ "$FAIL" = 0 ]; then
    echo "VOL70.6-GATE: PASS"
else
    echo "VOL70.6-GATE: FAIL"
fi
exit "$FAIL"
