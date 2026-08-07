#!/usr/bin/env bash
# v0.260.0（D5）：CI 换机可复现验证脚本（无人值守重建）。
#
# 依据：蓝图/14-阶段D-商用验收闸门.md D5「CI 换机可复现」——
# 全新机器上从零执行完整构建链，验证无人值守可复现。
#
# 覆盖（与 CI g0-fail-closed 步骤对应）：
#   1. 工具链就绪检查（rustup toolchain + targets + components）
#   2. 离线锁定解析（cargo metadata --locked --offline）
#   3. 全仓 clippy 零警告
#   4. 全仓测试零失败
#   5. unsafe SAFETY 注释 100%
#   6. intra-doc links 零
#   7. 发布物构建（image qemu-prod）
#   8. VRF-002 双构建字节一致
#
# 用法：bash tests/rebuild-ci.sh [--skip-tests]   （--skip-tests 跳过第 4 步加速）
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
cd "${REPO_ROOT}"

SKIP_TESTS=0
if [ "${1:-}" = "--skip-tests" ]; then
    SKIP_TESTS=1
fi

echo "[rebuild] 换机可复现验证开始（全新环境从零重建）"
FAILURES=0

step() {
    local name="$1"
    shift
    echo ""
    echo "======== [rebuild] ${name} ========"
    if ! "$@"; then
        echo "[rebuild] FAIL: ${name}"
        FAILURES=$((FAILURES + 1))
    else
        echo "[rebuild] PASS: ${name}"
    fi
}

# 1. 工具链就绪
step "toolchain (rustup show)" rustup show
step "cargo metadata --locked --offline" cargo metadata --locked --offline --format-version 1 > /dev/null

# 2. 格式（注意：本机 stable 无 group_imports（nightly 特性），fmt 门禁
# 以 CI nightly-2026-04-04 为权威；本脚本在 CI 环境执行该步骤）。
if [ -n "${RUSTUP_TOOLCHAIN:-}" ] && [ "${RUSTUP_TOOLCHAIN}" != "stable" ]; then
    step "cargo fmt --check" cargo fmt --all -- --check
else
    echo "[rebuild] SKIP: cargo fmt（本机 stable 无 group_imports；CI nightly 执行）"
fi

# 3. 全仓 clippy（fail-closed）
step "clippy -D warnings" bash -c "cargo clippy --workspace --all-targets --locked -- -D warnings 2>/dev/null"

# 4. 全仓测试
if [ "${SKIP_TESTS}" = "0" ]; then
    step "workspace tests" cargo test --workspace --locked
fi

# 5. 门禁
step "audit-unsafe-safety" cargo xtask audit-unsafe-safety
step "intra-doc links" bash -c "cargo doc --workspace --no-deps 2>&1 | grep -c 'unresolved link' | grep -q '^0$'"
step "ci-policy" cargo xtask test ci-policy

# 6. 发布物构建
step "image qemu-prod" cargo xtask image qemu-prod

# 7. VRF-002 确定性
step "build-compare (bit-identical)" cargo xtask build-compare

echo ""
if [ "${FAILURES}" -gt 0 ]; then
    echo "[rebuild] 结果：${FAILURES} 项失败 —— 换机不可复现"
    exit 1
fi
echo "[rebuild] 结果：全部通过 —— 换机可复现（无人值守）"
