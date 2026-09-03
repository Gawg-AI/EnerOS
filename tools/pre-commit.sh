#!/usr/bin/env bash
# ============================================================
# EnerOS / Power Native Agent OS — Pre-commit Hook
# Version: v0.3.1
# Target: WSL2 Ubuntu / native Linux (Bash)
# ============================================================
# Git pre-commit hook: runs the eneros-ci quality gate
# (fmt / clippy / deny / test) before allowing a commit.
#
# Blueprint: phase0.md §v0.2.0 (CI/CD pipeline)
#
# Install / refresh:
#   tools/pre-commit.sh install
#   （v0.3.0：安装时写入版本戳；hook 运行前与 tools/pre-commit.sh
#   比对——旧副本静默续用自检报错，杜绝 .git/hooks 陈旧副本。）
#
# 失败处置（63.3 WP-63.3.5：绕过门教唆文案删除——蓝图 62 §3.3 硬规则
# 「pre-commit 不绕过」）：
#   修复检查项后重试提交；hook 截断/网络抖动 → 重跑 `cargo run -p
#   eneros-ci` 确认真实失败面后修复，不绕过门。
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
HOOK_VERSION="v0.3.1"

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
    echo -e "${GREEN}[INFO]${NC} It will run 'cargo run -p eneros-ci' on each commit."
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

# --- default: pre-commit hook behavior ---
# Run the eneros-ci quality gate (fmt / clippy / deny / test).
if cargo run -p eneros-ci; then
    echo -e "${GREEN}✓ Pre-commit checks passed${NC}"
    exit 0
else
    echo -e "${RED}✗ Pre-commit checks failed${NC}" >&2
    echo -e "${RED}  修复上述检查项后重试提交。${NC}" >&2
    echo -e "${RED}  hook 截断/网络抖动（advisory fetch）→ 重跑 'cargo run -p eneros-ci'${NC}" >&2
    echo -e "${RED}  修复后重试；任何绕过门参数都在硬规则禁止之列（蓝图 62 §3.3）。${NC}" >&2
    exit 1
fi
