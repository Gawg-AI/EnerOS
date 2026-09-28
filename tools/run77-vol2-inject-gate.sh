#!/usr/bin/env bash
# ============================================================
# 蓝图 77 卷二 · 专属故障注入门（WP-77.2.1 / WP-77.2.2 §3 完成判据③）
#
# 三窗：
#   窗P（正例照跑）：正常 qemu-test UEFI 装配 → boot-manager 写 capsule
#       + 见证链（BOOT-WITNESS present）→ root 冻结 IDENTITY-VERDICT:
#       validated → IDENTITY-PUSH: OK / OTA READY / BOOT-IDENTITY-PAGE:
#       capsule。正常成功路径不得被硬失败或注入面污染。
#   窗N1（capsule-alloc）：注入占住固定页 + 写入上次 CRC 合法 capsule →
#       Address 预留失败 = `IDENTITY_CAPSULE` 结构化拒绝（引导关停）。
#       断言：rejection code 命中（harness --expect）、零 identity-capsule
#       写入行、零 IDENTITY-PUSH/OTA READY/BOOT-IDENTITY-PAGE、零
#       boot-evidence PASS；BOOTCTRL 分区介质逐字节不变（一次失败不得扣
#       trial——预留先于任何 prepare_attempt）。
#   窗N2（capsule-stale）：boot-manager 写入陈旧（boot_id/槽/代次均非
#       本次）但 CRC 合法的 capsule，见证真实 → root 六字段比较拒绝。
#       断言：IDENTITY-VERDICT: rejected、零 IDENTITY-PUSH: OK、零
#       OTA READY、BOOT-IDENTITY-PAGE: absent-or-rejected(zeros)、
#       root 照常心跳（拒绝不阻断运行时）。qemu-test 合成身份**不**兜底
#       （合成仅限 Absent）。
#
# 注入面与 release 构建互斥：image_factory 仅在
# ENEROS_BOOT_MANAGER_FAULT_INJECT=1 时启用 fault-inject feature（模式经
# ENEROS_FAULT_INJECT_MODE 构建期选定）；release 流水线恒缺省。
#
# 用法：DL_N1=240 DL_N2=420 tools/run77-vol2-inject-gate.sh
# 退出：FAIL=0 → VOL77.2-INJECT-GATE: PASS
# ============================================================
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
python3 "$ROOT/tools/criterion.py" --selftest >/dev/null || {
  echo "criterion.py selftest FAIL"; exit 9; }
cd "$ROOT"
source tools/gate-env.sh

EVROOT="$ROOT/evidence/77/vol2-inject"
RUN="$ROOT/target/evidence77-vol2-inject"
DL_N1="${DL_N1:-240}"
DL_N2="${DL_N2:-600}"
DL_P="${DL_P:-600}"
FAIL=0
TPL="$ROOT/target/evidence61/template.fd"

say() { echo "[77.2-inject] $*"; }
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

mkdir -p "$EVROOT" "$RUN"

# BOOTCTRL 分区字节提取（介质逐字节不变判据；qemu-v1: lba 18432 / 2048）。
dump_bootctrl() { # $1 = image, $2 = out
    dd if="$1" of="$2" bs=512 skip=18432 count=2048 status=none
}

TEST_ROOT_KEY_HEX="1f58396696deec97a0090fa612eb5c4784f0b114693696916c01e9cff8ebc5c7ec0c18caf02a7478c73e54bf13a06fb11624fd1a233ab5e41217aa7bb3803e04"

build_image() { # $1 = stage 名, $2 = 注入模式（空 = 正常装配）
    local stage="$1" mode="${2:-}"
    # qemu-test 装配的探针链与测试锚同源配对（vol7 同款）——dev 锚会使
    # OTA-CONFIG 域检拒绝、OTA READY 永不可达（门判据 P-7 依赖装配）。
    ENEROS_KMS_DIR=configs/kms-dev cargo xtask config compile --profile qemu-test \
        --accept-test-signing-key --root-key-hex "$TEST_ROOT_KEY_HEX" \
        >"$RUN/config-compile-$stage.log" 2>&1 || { echo "CONFIG-COMPILE FAIL"; return 1; }
    gate_relink_cached "$RUN/sel4-$stage.log" "sel4-eipc,qemu-test-synth" || return 1
    # 注入 env 导出后进 gate_build_stamp 的 ENEROS_ 环境签名——三镜像
    # 缓存互不串扰；gate_image_raw 是 bash 函数，只能经环境传递。
    if [ -n "$mode" ]; then
        export ENEROS_BOOT_MANAGER_FAULT_INJECT=1
        export ENEROS_FAULT_INJECT_MODE="$mode"
    fi
    gate_image_raw_cached qemu-test "$RUN/image-$stage.log" \
        "sel4-eipc,qemu-test-synth" || return 1
    unset ENEROS_BOOT_MANAGER_FAULT_INJECT ENEROS_FAULT_INJECT_MODE
    cp target/eneros/artifacts/qemu-test/eneros-qemu-test.raw "$RUN/eneros-$stage.raw"
}

[ -f "$TPL" ] || { say "vars template missing（先跑 vol5/61 段0 生成）"; exit 3; }

# ============================================================
say "=== 段1：三镜像构建（P=正例 / N1=capsule-alloc / N2=capsule-stale）==="
# ============================================================
build_image pos || { say "正例镜像构建失败"; exit 3; }
build_image n1alloc "capsule-alloc" || { say "N1 注入镜像构建失败"; exit 3; }
build_image n2stale "capsule-stale" || { say "N2 注入镜像构建失败"; exit 3; }

# ============================================================
say "=== 段2：窗N1 capsule-alloc（Address 预留失败 = 硬拒绝 + 介质不变）==="
# ============================================================
DIR_N1="$RUN/win-n1"; mkdir -p "$DIR_N1"; rm -f "$DIR_N1/qemu-serial.log"
cp "$RUN/eneros-n1alloc.raw" "$DIR_N1/boot.raw"
dump_bootctrl "$DIR_N1/boot.raw" "$DIR_N1/bootctrl.before"
cargo xtask run qemu-test \
    --image "$DIR_N1/boot.raw" --vars-template "$TPL" \
    --expect REJECT:IDENTITY_CAPSULE --evidence-dir "$DIR_N1" \
    --qmp-deadline-secs "$DL_N1" >"$DIR_N1/harness.log" 2>&1
RC_N1=$?
S_N1="$DIR_N1/qemu-serial.log"
dump_bootctrl "$DIR_N1/boot.raw" "$DIR_N1/bootctrl.after"

check N1-1 "结构化拒绝 REJECT:IDENTITY_CAPSULE（harness 判据）" bash -c "[ $RC_N1 -eq 0 ]"
check N1-2 "拒绝 JSON 在串口留档" bash -c "grep -aq 'eneros.boot-rejection.v1' '$S_N1' && grep -aq 'IDENTITY_CAPSULE' '$S_N1'"
check N1-3 "占页注入 armed 行在档" bash -c "grep -aq 'fault-inject capsule-alloc armed' '$S_N1'"
check N1-4 "零 capsule 写入（stage=identity-capsule 不出现）" bash -c "! grep -aq 'ENEROS stage=identity-capsule' '$S_N1'"
check N1-5 "零 root 侧身份推送/装配" bash -c "! grep -aqE 'IDENTITY-PUSH: OK|OTA READY' '$S_N1'"
check N1-6 "零身份页注入（BOOT-IDENTITY-PAGE 不出现）" bash -c "! grep -aq 'BOOT-IDENTITY-PAGE' '$S_N1'"
check N1-7 "零 boot-evidence PASS（引导确实关停）" bash -c "! grep -aq 'eneros.boot-evidence.v1' '$S_N1'"
check N1-8 "BOOTCTRL 介质逐字节不变（一次失败不扣 trial）" bash -c "cmp -s '$DIR_N1/bootctrl.before' '$DIR_N1/bootctrl.after'"
cp -f "$S_N1" "$EVROOT/n1.qemu-serial.log" 2>/dev/null || true
cp -f "$DIR_N1/harness.log" "$EVROOT/n1.harness.log" 2>/dev/null || true
sha256sum "$DIR_N1/bootctrl.before" "$DIR_N1/bootctrl.after" > "$EVROOT/n1.bootctrl.sha256" 2>/dev/null || true

# ============================================================
say "=== 段3：窗N2 capsule-stale（陈旧 capsule × 真实见证 = 拒身份）==="
# ============================================================
DIR_N2="$RUN/win-n2"; mkdir -p "$DIR_N2"; rm -f "$DIR_N2/qemu-serial.log"
cp "$RUN/eneros-n2stale.raw" "$DIR_N2/boot.raw"
cargo xtask run qemu-test \
    --image "$DIR_N2/boot.raw" --vars-template "$TPL" \
    --expect PASS --evidence-dir "$DIR_N2" \
    --ready-marker root-heartbeat --qmp-deadline-secs "$DL_N2" >"$DIR_N2/harness.log" 2>&1
RC_N2=$?
S_N2="$DIR_N2/qemu-serial.log"

# harness rc 不作判据（超时强杀 rc 恒负；vol5/vol7 先例 = 查串口标记）。
check N2-1 "引导照常完成（root 心跳就绪；拒绝不阻断运行时）" bash -c "grep -aq 'test\":\"root-heartbeat\",\"result\":\"PASS\"' '$S_N2'"
check N2-2 "boot-manager 已写 capsule（写入路径本身健康）" bash -c "grep -aq 'ENEROS stage=identity-capsule' '$S_N2'"
check N2-3 "见证链在档（BOOT-WITNESS present）" bash -c "grep -aq 'BOOT-WITNESS: present' '$S_N2'"
check N2-4 "六字段裁决拒绝（IDENTITY-VERDICT: rejected）" bash -c "grep -aq 'IDENTITY-VERDICT: rejected' '$S_N2'"
check N2-5 "零身份推送（IDENTITY-PUSH: OK 不出现）" bash -c "! grep -aq 'IDENTITY-PUSH: OK' '$S_N2'"
check N2-6 "零 OTA 装配（OTA READY 不出现）" bash -c "! grep -aq 'OTA READY' '$S_N2'"
check N2-7 "身份页零注入（absent-or-rejected(zeros)）" bash -c "grep -aq 'BOOT-IDENTITY-PAGE: absent-or-rejected(zeros)' '$S_N2'"
check N2-8 "合成身份不兜底拒绝态（synthetic 不出现）" bash -c "! grep -aq 'TEST assembly synthetic identity' '$S_N2'"
cp -f "$S_N2" "$EVROOT/n2.qemu-serial.log" 2>/dev/null || true

# ============================================================
say "=== 段4：窗P 正例照跑（正常 UEFI 装配全链健康）==="
# ============================================================
DIR_P="$RUN/win-pos"; mkdir -p "$DIR_P"; rm -f "$DIR_P/qemu-serial.log"
cp "$RUN/eneros-pos.raw" "$DIR_P/boot.raw"
cargo xtask run qemu-test \
    --image "$DIR_P/boot.raw" --vars-template "$TPL" \
    --expect PASS --evidence-dir "$DIR_P" \
    --reset-bootctrl --ready-marker root-heartbeat --qmp-deadline-secs "$DL_P" >"$DIR_P/harness.log" 2>&1
RC_P=$?
S_P="$DIR_P/qemu-serial.log"

check P-1 "引导完成（root 心跳就绪）" bash -c "grep -aq 'test\":\"root-heartbeat\",\"result\":\"PASS\"' '$S_P'"
check P-2 "capsule 写入（stage=identity-capsule）" bash -c "grep -aq 'ENEROS stage=identity-capsule' '$S_P'"
check P-3 "见证 verified + mailbox published（elfloader 0003）" bash -c "grep -aq 'boot witness verified' '$S_P'"
check P-4 "内核 extra chunk 发布（0004）" bash -c "grep -aq 'boot witness published to bootinfo extra chunk' '$S_P'"
check P-5 "root 见证在档（BOOT-WITNESS present）" bash -c "grep -aq 'BOOT-WITNESS: present' '$S_P'"
check P-6 "六字段裁决放行（IDENTITY-VERDICT: validated）" bash -c "grep -aq 'IDENTITY-VERDICT: validated' '$S_P'"
check P-7 "身份推送 + OTA 装配（IDENTITY-PUSH: OK / OTA READY）" bash -c "grep -aq 'IDENTITY-PUSH: OK' '$S_P' && grep -aq 'OTA READY' '$S_P'"
check P-8 "身份页真实注入（BOOT-IDENTITY-PAGE: capsule）" bash -c "grep -aq 'BOOT-IDENTITY-PAGE: capsule' '$S_P'"
check P-9 "零 PANIC" bash -c "! grep -aq 'PANIC' '$S_P'"
cp -f "$S_P" "$EVROOT/pos.qemu-serial.log" 2>/dev/null || true

# ============================================================
if [ "$FAIL" -eq 0 ]; then
    echo "VOL77.2-INJECT-GATE: PASS"
else
    echo "VOL77.2-INJECT-GATE: FAIL"
fi
exit $FAIL
