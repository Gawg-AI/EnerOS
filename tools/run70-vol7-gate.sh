#!/usr/bin/env bash
# ============================================================
# 蓝图 70.7 出口门（v0.337.0）：签名固件更新、BootControl 与重启回滚
#
# 装配：qemu-test（合成身份；root 探针直驱 OTA1 安装事务）。
#
# 双镜像：A（0.336.0 基线 epoch E）+ B（0.337.0 epoch E+3600）——
# COMBINED 启动盘 = A 的 BOOT_A + B 的 BOOT_A 载荷（注入 BOOT_B 分区）。
#
# 窗T-1：boot（reset-bootctrl）→ 探针 CHECK-DESC 武装 → BOOT 守卫查询 →
#   BOOT trial 事务（bootctrl 双副本写：B trial 严格高于 A，序列+1）。
# 窗T-2：同盘重启（无 reset，复用 T1 run vars 保 anchor）→ boot-manager
#   选择 B → 启动 B 载荷（slot=B + 新 manifest digest）→ 探针自适应确认
#   （B successful=true）→ 持久化。
#
# 用法：DL=700 tools/run70-vol7-gate.sh
# 退出：FAIL=0 → VOL70.7-GATE: PASS
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
source tools/gate-env.sh

EVROOT="$ROOT/evidence/70/70.7"
RUN="$ROOT/target/evidence70-vol7"
DL="${DL:-700}"
DL_T2="${DL_T2:-420}"
FAIL=0

mkdir -p "$EVROOT" "$RUN"
TPL="$ROOT/target/evidence61/template.fd"

say() { echo "[vol7-gate] $*"; }
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

wait_serial_stable() {
    local f="$1" prev=-1 cur=0
    for _ in $(seq 1 30); do
        cur=$(stat -c%s "$f" 2>/dev/null || echo 0)
        if [ "$cur" = "$prev" ]; then return 0; fi
        prev=$cur
        sleep 2
    done
}

# ============================================================
say "=== 段0：清理 ==="
# ============================================================
gate_cleanup_env 5577 5582 2404 1883 || true
sleep 1

# ============================================================
say "=== 段1：双镜像构建（A=0.336.0 epoch E / B=0.337.0 epoch E+3600）==="
# ============================================================
TEST_ROOT_KEY_HEX="1f58396696deec97a0090fa612eb5c4784f0b114693696916c01e9cff8ebc5c7ec0c18caf02a7478c73e54bf13a06fb11624fd1a233ab5e41217aa7bb3803e04"
EPOCH_A=$(cat target/evidence61/SOURCE_DATE_EPOCH 2>/dev/null || echo 1755523200)
EPOCH_B=$((EPOCH_A + 3600))
say "EPOCH_A=$EPOCH_A EPOCH_B=$EPOCH_B"

# 探针测试数据（token/签名对 + 候选配置束路径；qemu-test-synth 构建 env）。
PKI="$RUN/pki"; mkdir -p "$PKI"
openssl ecparam -name prime256v1 -genkey -noout -out "$PKI/station.key" 2>/dev/null \
    || { say "station key 生成失败"; exit 2; }
python3 - "$PKI/station.key" "$RUN/probe-token.hex" "$RUN/probe-sig.hex" <<'PYEOF'
import subprocess, sys, struct
key, tok_out, sig_out = sys.argv[1], sys.argv[2], sys.argv[3]
principal = b"station-ops".ljust(16, b"\0")
roles = bytes([2, 0, 0, 0])
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
assert len(sig) <= 72
open(tok_out, "w").write(token.hex())
open(sig_out, "w").write(sig.hex())
PYEOF
[ -s "$RUN/probe-token.hex" ] || { say "探针 token 生成失败"; exit 2; }
export ENEROS_MGT_PROBE_TOKEN="$(cat "$RUN/probe-token.hex")"
export ENEROS_MGT_PROBE_SIG="$(cat "$RUN/probe-sig.hex")"
export ENEROS_MGT_PROBE_BUNDLE="$RUN/config-bundle-gen2.bin"

# 基线束（gen1，镜像 A/B 嵌入）。
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
    --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" \
    > "$RUN/config-compile-a.log" 2>&1 || { say "基线 config compile 失败"; exit 2; }
# 候选束（gen2，70.6 配置事务回归载荷——内容同 gen1，代次区分）。
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
    --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" --generation 2 \
    > "$RUN/config-compile-b.log" 2>&1 || { say "候选 config compile 失败"; exit 2; }
cp product/config-bundle.bin "$RUN/config-bundle-gen2.bin"
ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
    --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" \
    > "$RUN/config-compile-restore.log" 2>&1 || { say "基线束恢复失败"; exit 2; }

# 根循环探针（qemu-test-synth）。
ROOT_TASK_FEATURES="sel4-eipc,qemu-test-synth" bash third_party/sel4-build-rust.sh \
    > "$RUN/sel4-t.log" 2>&1 || { say "sel4 重链接失败"; exit 3; }

# 镜像 A（workspace 0.336.0，epoch E）。
gate_image_raw qemu-test "$RUN/image-a.log" || { say "镜像 A 构建失败"; exit 3; }
RAW_A="$RUN/eneros-a.raw"
cp target/eneros/artifacts/qemu-test/eneros-qemu-test.raw "$RAW_A"

# 镜像 B：升版 0.337.0（S4 于本门内执行——A=0.336.0 已构建）+ epoch+3600。
cargo xtask bump-version --set 0.337.0 > "$RUN/bump-337.log" 2>&1 \
    || { say "bump 0.337.0 失败"; exit 2; }
export SOURCE_DATE_EPOCH="$EPOCH_B"
gate_image_raw qemu-test "$RUN/image-b.log" || { say "镜像 B 构建失败"; exit 3; }
unset SOURCE_DATE_EPOCH
RAW_B="target/eneros/artifacts/qemu-test/eneros-qemu-test.raw"

# COMBINED：A 的全盘 + B 的 BOOT_A 载荷注入 BOOT_B 分区（8 MiB @ lba 36864）。
COMBINED="$RUN/eneros-combined.raw"
cp "$RAW_A" "$COMBINED"
dd if="$RAW_B" of="$COMBINED" bs=512 skip=20480 seek=36864 count=16384 conv=notrunc status=none \
    || { say "BOOT_B 注入失败"; exit 3; }
sha256sum "$COMBINED" | awk '{print $1}' > "$EVROOT/combined.sha256"
say "COMBINED 就绪（BOOT_A=A 载荷 / BOOT_B=B 载荷）"

# ============================================================
say "=== 段2：窗T-1 安装事务实弹（探针直驱）==="
# ============================================================
DIR="$RUN/t1"
rm -rf "$DIR"; mkdir -p "$DIR"
DATADISK="$RUN/data-disk.raw"
rm -f "$DATADISK"; truncate -s 16M "$DATADISK"
cargo xtask run qemu-test \
    --image "$COMBINED" --vars-template "$TPL" \
    --expect PASS --evidence-dir "$DIR" \
    --data-disk "$DATADISK" \
    --net-mode present --net-user --net-protocols present \
    --reset-bootctrl --qmp-shutdown --ready-marker probe-complete --qmp-deadline-secs "$DL" &
HPID=$!

S="$DIR/qemu-serial.log"
for i in $(seq 1 300); do
    if grep -aq "PRODUCT-READY" "$S" 2>/dev/null; then break; fi
    if ! kill -0 $HPID 2>/dev/null; then break; fi
    sleep 1
done
for i in $(seq 1 240); do
    if grep -aq "MGT-PROBE: boot trial tx sent" "$S" 2>/dev/null; then break; fi
    if ! kill -0 $HPID 2>/dev/null; then break; fi
    sleep 1
done
sleep 20

kill -9 $(pgrep -f qemu-system) 2>/dev/null || true
sleep 3
kill $HPID 2>/dev/null || true
wait_serial_stable "$S"
cp "$S" "$EVROOT/t1.qemu-serial.log" 2>/dev/null || true

# ---------- 窗T-1 判据 ----------
say "=== 段2 判据（窗T-1）==="
check R1 "零 PANIC / 零分配失败" bash -c "! grep -aqcE 'PANIC|memory allocation of' '$EVROOT/t1.qemu-serial.log'"
check R2 "PRODUCT-READY 达成" bash -c "grep -aq 'PRODUCT-READY: true' '$EVROOT/t1.qemu-serial.log'"
check R3 "BOOT 守卫查询（0x0A）" bash -c "grep -aq 'BOOT-GUARD: inactive_lba=' '$EVROOT/t1.qemu-serial.log'"
check R4 "BOOT trial 事务提交（bootctrl 双副本+flush）" bash -c "grep -aq 'BOOT-TRIAL: tx committed seq=' '$EVROOT/t1.qemu-serial.log'"

# ============================================================
say "=== 段3：窗T-2 同盘重启进入新载荷（无 --reset-bootctrl）==="
# ============================================================
DIR2="$RUN/t2"
rm -rf "$DIR2"; mkdir -p "$DIR2"
# 复用 T1 的 run vars（boot-manager protected anchor 存于 UEFI vars——
# 模板重置 = anchor 丢失 = BOOTCTRL 拒启；70.6 run14/15 实锤）。
VARS_T2="$DIR/AAVMF_VARS.run.fd"
[ -f "$VARS_T2" ] || { say "缺 T1 run vars"; exit 2; }
cargo xtask run qemu-test \
    --image "$COMBINED" --vars-template "$VARS_T2" \
    --expect PASS --evidence-dir "$DIR2" \
    --data-disk "$DATADISK" \
    --net-mode present --net-user --net-protocols present \
    --qmp-shutdown --ready-marker probe-complete --qmp-deadline-secs "$DL_T2" &
HPID2=$!
S2="$DIR2/qemu-serial.log"
for i in $(seq 1 300); do
    if grep -aq "PRODUCT-READY" "$S2" 2>/dev/null; then break; fi
    if ! kill -0 $HPID2 2>/dev/null; then break; fi
    sleep 1
done
for i in $(seq 1 180); do
    if grep -aq "MGT-PROBE: sequence complete" "$S2" 2>/dev/null; then break; fi
    if ! kill -0 $HPID2 2>/dev/null; then break; fi
    sleep 1
done
sleep 15

kill -9 $(pgrep -f qemu-system) 2>/dev/null || true
sleep 3
kill $HPID2 2>/dev/null || true
wait_serial_stable "$S2"
cp "$S2" "$EVROOT/t2.qemu-serial.log" 2>/dev/null || true

# ---------- 窗T-2 判据 ----------
say "=== 段3 判据（窗T-2）==="
check R5 "零 PANIC / 零分配失败" bash -c "! grep -aqcE 'PANIC|memory allocation of' '$EVROOT/t2.qemu-serial.log'"
check R6 "PRODUCT-READY 达成" bash -c "grep -aq 'PRODUCT-READY: true' '$EVROOT/t2.qemu-serial.log'"
check R7 "boot-evidence slot=B（boot-manager 选择非活动槽）" bash -c "grep -aq 'boot-evidence.v1..result...PASS..slot...B' '$EVROOT/t2.qemu-serial.log'"
check R8 "boot_reason=UPDATE_TRIAL（跨重启试启动语义）" bash -c "grep -aq 'UPDATE_TRIAL' '$EVROOT/t2.qemu-serial.log'"
check R9 "BOOT-CONFIRM 自适应确认（slot=B → successful）" bash -c "grep -aq 'BOOT-CONFIRM: slot=B' '$EVROOT/t2.qemu-serial.log'"
check R10 "确认持久化（序列推进）" bash -c "grep -aq 'BOOT-CONFIRM: slot=B secver=' '$EVROOT/t2.qemu-serial.log'"

# ============================================================
if [ "$FAIL" = 0 ]; then
    echo "VOL70.7-GATE: PASS"
else
    echo "VOL70.7-GATE: FAIL"
fi
exit "$FAIL"
