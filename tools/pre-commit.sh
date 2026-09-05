#!/usr/bin/env bash
# ============================================================
# EnerOS / Power Native Agent OS — Pre-commit Hook
# Version: v0.4.0
# Target: WSL2 Ubuntu / native Linux (Bash)
# ============================================================
# Git pre-commit hook：三快门（蓝图 67 WP-67.1.4 / R-67.8）：
#   1. cargo xtask qa gate sign-keys   （签名钥显式来源；WP-67.1.1 迁 xtask）
#   2. cargo xtask qa gate mock-leak   （mock 收编宽面门）
#   3. cargo xtask qa gng-version      （Go/No-Go 与版本一致性）
# fmt / clippy / deny / test 四项归 CI（.github/workflows/ci.yml 直跑；
# 原 eneros-ci crate 已于蓝图 67 WP-67.1.4 退役——门禁不变式每条一个
# owner，本地三快门 + CI 四项，无双实现）。
#
# Blueprint: 蓝图/67-接缝收编与守卫接电蓝图（WP-67.1）
#
# Install / refresh:
#   tools/pre-commit.sh install
#   （v0.3.0：安装时写入版本戳；hook 运行前与 tools/pre-commit.sh
#   比对——旧副本静默续用自检报错，杜绝 .git/hooks 陈旧副本。）
#
# 失败处置（蓝图 62 §3.3 硬规则「pre-commit 不绕过」）：
#   修复对应门后重试提交；hook 截断 → 重跑上述三门确认真实失败面，
#   不绕过门。
# ============================================================

set -euo pipefail

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# 仓根解析三分支（63.3 审查修正：`git -C .git/hooks rev-parse` 在 GIT_DIR
# 内返回 .git 自身——不能按 SCRIPT_DIR 解析）：
#   ① hook 模式：git 运行 hook 时 cwd=工作树根 → 直接 rev-parse；
#   ② direct 调用 tools/ 副本：SCRIPT_DIR 上一级；
#   ③ 兜底：.git/hooks 副本上两级。
if REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    :
elif [ -f "$SCRIPT_DIR/pre-commit.sh" ]; then
    REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
else
    REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
fi
HOOK_PATH="$REPO_ROOT/.git/hooks/pre-commit"
SOURCE_HOOK="$REPO_ROOT/tools/pre-commit.sh"
HOOK_VERSION="v0.4.0"

# Color output (only when stdout is a terminal)
if [ -t 1 ]; then
    GREEN='\033[32m'
    RED='\033[31m'
    NC='\033[0m'
else
    GREEN=''
    RED=''
    NC=''
fi

# --- install subcommand: deploy hook into .git/hooks/ ---
if [ "${1:-}" = "install" ]; then
    mkdir -p "$REPO_ROOT/.git/hooks"
    cp "$SCRIPT_DIR/pre-commit.sh" "$HOOK_PATH"
    chmod +x "$HOOK_PATH"
    echo -e "${GREEN}[INFO]${NC} Pre-commit hook installed to: $HOOK_PATH ($HOOK_VERSION)"
    echo -e "${GREEN}[INFO]${NC} It will run the three fast gates (sign-keys / mock-leak / gng-version) on each commit."
    exit 0
fi

# --- hook freshness self-check（v0.3.0）：.git/hooks 副本与 tools/ 源
# 逐字节比对——旧副本静默续用 = 门语义与仓内纪律漂移（63.3 登记缺口）。
if ! cmp -s "$SOURCE_HOOK" "$HOOK_PATH" 2>/dev/null; then
    echo -e "${RED}✗ Stale pre-commit hook detected${NC}" >&2
    echo -e "${RED}  .git/hooks/pre-commit 与 tools/pre-commit.sh 不一致（旧副本静默续用）。${NC}" >&2
    echo -e "${RED}  刷新：tools/pre-commit.sh install${NC}" >&2
    exit 1
fi

# --- default: pre-commit hook behavior（v0.4.0 三快门）---
GATES=(
    "cargo xtask qa gate sign-keys"
    "cargo xtask qa gate mock-leak"
    "cargo xtask qa gng-version"
)
FAILED_GATE=""
for gate in "${GATES[@]}"; do
    if ! $gate; then
        FAILED_GATE="$gate"
        break
    fi
done

if [ -z "$FAILED_GATE" ]; then
    echo -e "${GREEN}✓ Pre-commit checks passed（sign-keys / mock-leak / gng-version）${NC}"
    exit 0
else
    echo -e "${RED}✗ Pre-commit checks failed（$FAILED_GATE）${NC}" >&2
    echo -e "${RED}  修复上述门后重试提交；重跑：${NC}" >&2
    echo -e "${RED}    cargo xtask qa gate sign-keys${NC}" >&2
    echo -e "${RED}    cargo xtask qa gate mock-leak${NC}" >&2
    echo -e "${RED}    cargo xtask qa gng-version${NC}" >&2
    echo -e "${RED}  任何绕过门参数都在硬规则禁止之列（蓝图 62 §3.3）。${NC}" >&2
    exit 1
fi
