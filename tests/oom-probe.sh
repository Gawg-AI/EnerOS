#!/usr/bin/env bash
set -euo pipefail

# 22 §2.W1.1 验收 3：构造堆耗尽 → #[alloc_error_handler] 触发而非 abort。
# aarch64 裸机探针（crates/runtime/oom-probe，零依赖），QEMU -machine virt
# -kernel 直跑（无需 seL4 完整系统）。
#
# 期望输出（断言标记）：
#   [oom-probe] alloc_error_handler triggered #1 layout=size:1000,align:1 — suspending ...
# 退出码 124（timeout 杀 QEMU）属预期：handler 按设计挂起（fail-closed），
# 不 abort、不退出。标记出现 = 通过。

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PROBE_DIR="${REPO_ROOT}/crates/runtime/oom-probe"
MARK="[oom-probe] alloc_error_handler triggered"

if [[ -n "${ENEROS_QEMU_BIN:-}" ]]; then
    QEMU_BIN="${ENEROS_QEMU_BIN}"
elif command -v qemu-system-aarch64 >/dev/null 2>&1; then
    QEMU_BIN="$(command -v qemu-system-aarch64)"
else
    QEMU_BIN="/home/ahx/.cache/xpack-qemu-arm-9.2.4-1/bin/qemu-system-aarch64"
fi

echo "==> building oom-probe (aarch64-unknown-none)"
# 注：crate 级 .cargo/config.toml 不被 workspace 构建读取（cargo 只读 CWD
# 链的 config），故在 probe 目录内构建并用 RUSTFLAGS 注入 linker script。
cd "${PROBE_DIR}"
RUSTFLAGS="-C panic=abort -C link-arg=-T$(pwd)/linker.ld -C link-arg=--gc-sections" \
    cargo build --target aarch64-unknown-none \
    -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem

echo "==> running probe in QEMU (timeout 15s; exit 124 expected)"
set +e
timeout 15 "${QEMU_BIN}" -machine virt -cpu cortex-a57 -m 64M -nographic \
    -kernel "${REPO_ROOT}/target/aarch64-unknown-none/debug/oom-probe" \
    > /tmp/oom-probe-run.log 2>&1
RC=$?
set -e
cat /tmp/oom-probe-run.log

if ! grep -qF "${MARK}" /tmp/oom-probe-run.log; then
    echo "FAIL: OOM handler mark missing (expected '${MARK}')" >&2
    exit 1
fi
if grep -q "aborting" /tmp/oom-probe-run.log; then
    echo "FAIL: abort observed" >&2
    exit 1
fi
echo "PASS: heap exhaustion → alloc_error_handler triggered, not abort (exit=${RC} expected 124)"
