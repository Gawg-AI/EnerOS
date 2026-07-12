#!/usr/bin/env bash
# ============================================================
# EnerOS / Power Native Agent OS — Pre-commit Hook
# Version: v0.2.0
# Target: WSL2 Ubuntu / native Linux (Bash)
# ============================================================
# Git pre-commit hook: runs the eneros-ci quality gate
# (fmt / clippy / deny / test) before allowing a commit.
#
# Blueprint: phase0.md §v0.2.0 (CI/CD pipeline)
#
# Install:
#   tools/pre-commit.sh install
#
# Bypass (not recommended):
#   git commit --no-verify
# ============================================================

set -euo pipefail

# Paths
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
HOOK_PATH="$REPO_ROOT/.git/hooks/pre-commit"

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
    echo -e "${GREEN}[INFO]${NC} Pre-commit hook installed to: $HOOK_PATH"
    echo -e "${GREEN}[INFO]${NC} It will run 'cargo run -p eneros-ci' on each commit."
    exit 0
fi

# --- default: pre-commit hook behavior ---
# Run the eneros-ci quality gate (fmt / clippy / deny / test).
if cargo run -p eneros-ci; then
    echo -e "${GREEN}✓ Pre-commit checks passed${NC}"
    exit 0
else
    echo -e "${RED}✗ Pre-commit checks failed${NC}" >&2
    echo -e "${RED}  Fix the issues above, or bypass with: git commit --no-verify${NC}" >&2
    exit 1
fi
