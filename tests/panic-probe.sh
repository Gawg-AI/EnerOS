#!/usr/bin/env bash
set -euo pipefail

# 22 §2.W1.2 验收 2（QEMU 侧）：注入 panic → panic handler 执行完整序列
# （审计逻辑 host 侧已测；此处验证目标侧：打印 PANIC + 武装 SP805 + 挂起）。
#
# 环境限制（ADR-026）：xpack QEMU 9.2.4 无 SP805/PSCI 设备——"超时→硬件
# 复位"动作为真机/QEMU 完整版验证项；本探针验证武装序列执行与 fail-closed
# 挂起（无 abort）。
#
# 判定：marker "[panic-probe] arming watchdog reset" 出现 ≥1 次 且日志
# 含 PANIC 标记 = panic handler 到达武装步骤（未 abort）。

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROBE_DIR="${REPO_ROOT}/crates/runtime/oom-probe"
MARK="[panic-probe] arming watchdog reset"

if [[ -n "${ENEROS_QEMU_BIN:-}" ]]; then
    QEMU_BIN="${ENEROS_QEMU_BIN}"
elif command -v qemu-system-aarch64 >/dev/null 2>&1; then
    QEMU_BIN="$(command -v qemu-system-aarch64)"
else
    # 34 GV-20：无 PATH 中的 QEMU → fail-closed（旧实现硬编码个人主机
    # 路径；换机即碎）。显式要求 ENEROS_QEMU_BIN。
    if [ -n "${ENEROS_QEMU_BIN:-}" ] && [ -x "${ENEROS_QEMU_BIN}" ]; then
        QEMU_BIN="${ENEROS_QEMU_BIN}"
    else
        echo "qemu-system-aarch64 not found in PATH; set ENEROS_QEMU_BIN" >&2
        exit 2
    fi
fi

echo "==> building panic-probe (aarch64-unknown-none)"
cd "${PROBE_DIR}"
RUSTFLAGS="-C panic=abort -C link-arg=-T$(pwd)/linker.ld -C link-arg=--gc-sections" \
    cargo build --bin panic-probe --target aarch64-unknown-none \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem

echo "==> running panic-probe in QEMU (timeout 15s; fail-closed hang expected)"
set +e
timeout 15 "${QEMU_BIN}" -machine virt -cpu cortex-a57 -m 64M -nographic \
    -kernel "${REPO_ROOT}/target/aarch64-unknown-none/debug/panic-probe" \
    > /tmp/panic-probe-run.log 2>&1
RC=$?
set -e
cat /tmp/panic-probe-run.log

COUNT=$(grep -cF "${MARK}" /tmp/panic-probe-run.log || true)
if [[ "${COUNT}" -lt 1 ]] || ! grep -q "PANIC:" /tmp/panic-probe-run.log; then
    echo "FAIL: panic handler 未到达武装步骤（marker=${COUNT}）" >&2
    exit 1
fi
echo "PASS: panic → handler 审计序列执行 → SP805 武装 → fail-closed 挂起 (qemu exit=${RC})"
echo "NOTE: 硬件复位（SP805 超时→SoC reset）为真机/QEMU-完整版验证项（ADR-026）"
