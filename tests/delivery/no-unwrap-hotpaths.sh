#!/usr/bin/env bash
# 21-B D1：生产热路径 unwrap/expect 纪律门禁（fail-closed）。
#
# 规则（热路径 = crates/protocols, crates/security, crates/drivers）：
# 对每个非测试 .rs 文件，用括号配平精确裁剪：
#   - doc 注释行（//! / ///）内的示例；
#   - 测试上下文（`#[cfg(test)]` 块、`mod tests` / `mod *_tests` 块、
#     以及 tests/ 目录与 *_tests.rs / tests.rs 模块文件）。
# 剩余生产代码中的每个 `unwrap()`/`expect(` 必须在前 8 行内有
# `// Reason:` 豁免注释（不可达不变量，21-B D1 允许）。
# 任一违规 → exit 1（CI 红）。
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
exec python3 - "$ROOT" <<'PY'
import os
import re
import sys

root = sys.argv[1]
hot_paths = [os.path.join(root, "crates/protocols"),
             os.path.join(root, "crates/security"),
             os.path.join(root, "crates/drivers")]
violations = []

UNWRAP = re.compile(r"\.unwrap\(\)|\.expect\(")
TEST_MOD = re.compile(r"^\s*mod\s+\w*tests?\s*\{")
CFG_TEST = re.compile(r"#\[cfg\(test\)\]")


def production_lines(lines):
    """括号配平裁剪测试块，返回 (行号1-based列表, 每行文本) 中生产区行。"""
    depth = 0
    in_test_block = False
    test_depth = 0
    prod = []
    for i, line in enumerate(lines, start=1):
        opens = line.count("{") - line.count("}")
        if not in_test_block:
            if CFG_TEST.search(line) or TEST_MOD.search(line):
                in_test_block = True
                test_depth = depth + (1 if "{" in line else 0)
                if "{" not in line:
                    # #[cfg(test)] 单独一行，下一行是 mod/impl 块头
                    pass
        if in_test_block:
            depth += opens
            if depth < test_depth:
                # 闭合到低于测试块起始深度 → 测试块结束
                in_test_block = False
                depth = 0
            continue
        depth += opens
        prod.append((i, line))
    return prod


def check_file(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().splitlines()
    prod = production_lines(lines)
    prod_by_line = dict(prod)
    for i, line in prod:
        if UNWRAP.search(line):
            trimmed = line.lstrip()
            if trimmed.startswith("//!") or trimmed.startswith("///"):
                continue  # doc 注释示例
            # 前 8 行找 Reason 豁免
            ok = False
            for j in range(max(1, i - 8), i):
                if "Reason:" in prod_by_line.get(j, ""):
                    ok = True
                    break
            if not ok:
                violations.append(f"{path}:{i}: {trimmed[:90]}")


def main():
    for base in hot_paths:
        for dirpath, dirnames, filenames in os.walk(base):
            if "tests" in dirpath.split(os.sep):
                continue
            for fn in filenames:
                if not fn.endswith(".rs"):
                    continue
                if fn.endswith("_tests.rs") or fn == "tests.rs":
                    continue
                if fn == "build.rs":
                    continue  # 构建脚本（build-dependencies）非热路径
                check_file(os.path.join(dirpath, fn))
    if violations:
        print(f"D1 gate: FAIL ({len(violations)} violations)")
        for v in violations[:40]:
            print(f"  {v}")
        sys.exit(1)
    print("D1 gate: PASS（生产热路径 unwrap/expect 全部带 Reason 豁免或处于测试上下文）")


main()
PY
