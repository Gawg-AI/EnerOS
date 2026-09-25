#!/usr/bin/env python3
"""门判据器（蓝图 73 批B）：tools/gate-criteria.toml 的唯一执行面。

子命令：
  check <id> <file>… [--count-only] [--echo-ok T] [--echo-fail T]
      读登记 → 全文件域逐 pattern 计数 → 判定 → 打印回声行 → exit。
      正常判定：exit 0（✓）/ 1（✗）；缺输入（on_missing_input="fail" 且
      文件不存在）：check 模式立即 ✗（文案含 input missing，exit 1），
      --count-only 模式 exit 3（stderr 注明，stdout 不出计数——调用方
      对 exit 3 直接 FAIL，见 r10-held B4）。
      --count-only：只输出 any_of 合并计数（B4 特例——N6 ≤ N 比较留在
      脚本）；--echo-ok/--echo-fail 覆写登记模板（多消费方条目回声在调用
      点，C2 逐字节同形）。
  --selftest
      stdlib unittest：恶意 fixture 红 / 好 fixture 绿 / 缺输入红 /
      全 token 拒截断与粘连 / 大小写敏感 / 回声逐字节 / py↔rust
      gate_terminal_status 共享 fixture 锁（testdata 文件在场时）。

边界（C3 全 token/锚定的实现语义）：
  左边界——命中起点前是行首或非 [A-Za-z0-9_:-] 字符（"BOOT-…"、"X…"
  粘连不命中）；右边界——**pattern 以词字符结尾时**，命中终点后是行尾
  或非 [A-Za-z0-9_:] 字符（"OKAY"、"gen=X" 之外的粘连不命中）；pattern
  以分隔符结尾时右边界开放（该分隔符本身即边界）：key 形
  （"OTA-CONFIG gen=" / "ACQ point="）值为后续字符，词形 "PANIC #"
  受纳 "PANIC #1 at …"（73 批C：本仓 panic 出口的非标准形）。
  大小写敏感默认（登记条目可显式 case_sensitive；当前全部 true）。
"""

from __future__ import annotations

import re
import sys
import tomllib
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
ROOT = TOOLS.parent
REGISTRY = TOOLS / "gate-criteria.toml"
FALSE_GREEN = TOOLS / "testdata" / "gate-false-green"
TERMINAL_FIXTURES = TOOLS / "testdata" / "gate-terminal-fixtures.txt"

# 左/右边界字符类（C3）：见模块 docstring。
_LEFT_CLASS = "A-Za-z0-9_:-"
_RIGHT_CLASS = "A-Za-z0-9_:"


def load_criteria() -> dict:
    with open(REGISTRY, "rb") as fh:
        return tomllib.load(fh)


def get_criterion(cid: str) -> dict:
    for entry in load_criteria().get("criterion", []):
        if entry.get("id") == cid:
            return entry
    raise SystemExit(f"criterion.py: 未登记的判据 id '{cid}'（{REGISTRY}）")


def match_count(line: str, pattern: str) -> int:
    """单行的全 token 命中计数（左边界恒锚定；右边界按尾字符性质决定）。

    公开面：run70_gate_judge.py 等消费方经此复用**同一**锚定语义，
    不再自带第二套匹配规则。

    右边界只在 pattern **以词字符结尾**时施加：以分隔符结尾的 pattern
    （key 形 `"…gen="`、词形 `"PANIC #"`）自身已经带边界，再要求后随
    非词字符会把 `"PANIC #1 at …"` 这类真实 panic 判成不命中（73 批C
    归档重放实证）。尾字符非词 ⇒ 右开放，粘连防护仍由左边界承担。
    """
    rx = f"(?<![{_LEFT_CLASS}])" + re.escape(pattern)
    if pattern[-1:].isalnum() or pattern[-1:] == "_":
        rx += f"(?![{_RIGHT_CLASS}])"
    return len(re.findall(rx, line))


def count_in_file(path: str, patterns: list[str], case_sensitive: bool = True) -> int:
    total = 0
    with open(path, errors="replace") as fh:
        for raw in fh:
            line = raw if case_sensitive else raw.lower()
            for pat in patterns:
                total += match_count(line, pat if case_sensitive else pat.lower())
    return total


def _render(template: str, count: int) -> str:
    return template.replace("{count}", str(count)).replace("{n}", str(count))


def run_check(
    cid: str,
    files: list[str],
    count_only: bool = False,
    echo_ok: str | None = None,
    echo_fail: str | None = None,
) -> tuple[int, str]:
    """执行一条判据。返回 (exit_code, stdout_message)。"""
    entry = get_criterion(cid)
    on_missing = entry.get("on_missing_input", "fail")
    case_sensitive = entry.get("case_sensitive", True)
    any_of = list(entry.get("any_of", []))
    none_of = list(entry.get("none_of", []))
    min_count = int(entry.get("min_count", 1))

    missing = [f for f in files if not Path(f).is_file()]
    if missing and on_missing == "fail":
        if count_only:
            return 3, ""
        note = f"（input missing: {missing[0]}）"
        tmpl = echo_fail if echo_fail is not None else entry.get(
            "echo_fail", f"[✗][{cid}] 判据不满足"
        )
        return 1, _render(tmpl, 0) + note

    got = sum(count_in_file(f, any_of, case_sensitive) for f in files)
    if count_only:
        return 0, str(got)

    bad = sum(count_in_file(f, none_of, case_sensitive) for f in files)
    ok = bad == 0 and got >= min_count
    if ok:
        tmpl = echo_ok if echo_ok is not None else entry.get(
            "echo_ok", f"[✓][{cid}] 判据满足（×{got}）"
        )
    else:
        tmpl = echo_fail if echo_fail is not None else entry.get(
            "echo_fail", f"[✗][{cid}] 判据不满足"
        )
    # 回声计数：✓ = any_of 命中数；✗ 且有违例 = 违例数（PANIC=N 语义与
    # 收编前脚本的 $PANICS 计数一致——C2 同形含数值语义）；否则 any_of 数。
    echo_count = bad if (not ok and bad > 0) else got
    return (0 if ok else 1), _render(tmpl, echo_count)


def main(argv: list[str]) -> int:
    if "--selftest" in argv:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(SelfTest)
        runner = unittest.TextTestRunner(verbosity=2)
        return 0 if runner.run(suite).wasSuccessful() else 1
    count_only = False
    positional: list[str] = []
    rest = argv[1:]
    i = 0
    echo_ok = echo_fail = None
    while i < len(rest):
        a = rest[i]
        if a == "--count-only":
            count_only = True
        elif a in ("--echo-ok", "--echo-fail"):
            if i + 1 >= len(rest):
                print(f"criterion.py: {a} 需要一个值", file=sys.stderr)
                return 2
            if a == "--echo-ok":
                echo_ok = rest[i + 1]
            else:
                echo_fail = rest[i + 1]
            i += 1
        elif a.startswith("--"):
            print(f"criterion.py: 未知旗标 {a}", file=sys.stderr)
            return 2
        else:
            positional.append(a)
        i += 1
    if positional and positional[0] == "check":
        positional = positional[1:]
    if len(positional) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    cid, files = positional[0], positional[1:]
    code, msg = run_check(cid, files, count_only, echo_ok, echo_fail)
    if msg:
        print(msg)
    return code


class SelfTest(unittest.TestCase):
    """S2/S4：恶意 fixture 红、好 fixture 绿、缺输入红、全 token、大小写。"""

    GOOD_LINES = (
        "[eneros-root-task] IDENTITY-PUSH: OK (slot=A gen=2 secver=1 floor=1 "
        "ver=0.339.0 — svc-ota assembled)\n"
        "[eneros-root-task] OTA-CONFIG gen=2 origin=config-bundle "
        "(svc-ota → svc-config grant OK)\n"
        "[svc-control-dispatch] EXEC-JOURNAL applied-confirmed intent=83 seq=7\n"
        "[svc-modbus-tcp] ACQ point=400 value=400.0 q=good\n"
    )

    def setUp(self):
        import tempfile

        self.tmp = tempfile.TemporaryDirectory()
        self.good = str(Path(self.tmp.name, "good.log"))
        with open(self.good, "w") as fh:
            fh.write(self.GOOD_LINES)

    def tearDown(self):
        self.tmp.cleanup()

    def test_malicious_fixture_r9_red(self):
        for name in ("r9-only-boot-identity.log", "r9-honest-failure.log"):
            code, msg = run_check("VOL70.5-R9", [str(FALSE_GREEN / name)])
            self.assertEqual(code, 1, f"{name}: 恶意卷宗必须 ✗（得到 {msg!r}）")

    def test_honest_failure_names_none_of(self):
        code, msg = run_check("VOL70.5-R9", [str(FALSE_GREEN / "r9-honest-failure.log")])
        self.assertEqual(code, 1)
        # 失败原因是 none_of（诚实失败行），不只是 any_of 缺席。
        self.assertEqual(
            count_in_file(str(FALSE_GREEN / "r9-honest-failure.log"), ["IDENTITY-PUSH: FAILED"]), 1
        )

    def test_good_fixture_green_and_echo_bytes(self):
        code, msg = run_check("VOL70.5-R9", [self.good])
        self.assertEqual(code, 0)
        self.assertEqual(msg, "[✓][R9] 窗B 身份面观测（IDENTITY ×1）")
        code, msg = run_check("VOL70.5-R9b", [self.good])
        self.assertEqual((code, msg), (0, "[✓][R9b] 窗B 配置取数（OTA-CONFIG ×1）"))

    def test_synthetic_identity_alternative_matches(self):
        p = str(Path(self.tmp.name, "synth.log"))
        with open(p, "w") as fh:
            fh.write(
                "[eneros-root-task] IDENTITY: capsule absent — TEST assembly "
                "synthetic identity (qemu-test-synth, non-production)\n"
            )
        code, _ = run_check("VOL70.5-R9", [p])
        self.assertEqual(code, 0, "synthetic 备选 token 必须独立满足 R9")

    def test_missing_input_red(self):
        code, msg = run_check("VOL70.5-R9", ["/nonexistent/serial.log"])
        self.assertEqual(code, 1)
        self.assertIn("input missing", msg)
        code, msg = run_check("R10-B4-DELIVERY", ["/nonexistent/serial.log"], count_only=True)
        self.assertEqual((code, msg), (3, ""), "count-only 缺输入 = exit 3 零计数")

    def test_full_token_rejects_glued_and_truncated(self):
        p = str(Path(self.tmp.name, "glue.log"))
        with open(p, "w") as fh:
            fh.write(
                "XIDENTITY-PUSH: OK glue\n"
                "IDENTITY-PUSH: OKAY trunc\n"
                "BOOT-IDENTITY-PUSH: OK family\n"
                "XOTA-CONFIG gen=2 glue\n"
            )
        self.assertEqual(run_check("VOL70.5-R9", [p])[0], 1, "粘连/截断形不得命中")
        self.assertEqual(
            count_in_file(p, ["IDENTITY-PUSH: OK"]), 0, "OKAY 与 BOOT- 前缀都不是命中"
        )
        self.assertEqual(count_in_file(p, ["OTA-CONFIG gen="]), 0, "XOTA- 粘连不命中")
        # 真 key 形（尾缀 '=' 右开放）在 good fixture 里已证命中。

    def test_case_sensitive_panic_scan(self):
        p = str(Path(self.tmp.name, "case.log"))
        with open(p, "w") as fh:
            fh.write("quiet boot; panic: panicked at nowhere (lowercase)\n")
        code, _ = run_check("PANIC-SCAN", [p])
        self.assertEqual(code, 0, "小写 panic 不得命中（大小写敏感）")
        with open(p, "a") as fh:
            fh.write("PANIC: panicked at src/main.rs:1:1\n")
        code, msg = run_check(
            "PANIC-SCAN", [p], echo_ok="[✓][B5] 零 PANIC", echo_fail="[✗][B5] PANIC={count}"
        )
        self.assertEqual((code, msg), (1, "[✗][B5] PANIC=1"))

    def test_panic_scan_full_file_domain(self):
        p = str(Path(self.tmp.name, "pre-ready.log"))
        with open(p, "w") as fh:
            fh.write("PRODUCT-READY not yet\nPANIC: panicked at boot\n...600 healthy lines\n")
        code, _ = run_check("PANIC-SCAN", [p])
        self.assertEqual(code, 1, "启动段（READY 之前）PANIC 必须计入——全文件域")

    def test_panic_scan_hash_form_emitters_red(self):
        """本仓每个 panic 出口词形都必须被 PANIC-SCAN 词表覆盖。

        73 批C 归档重放实证（125 份 qemu-serial 日志）：旧词表只认 Rust
        标准形 `PANIC: panicked at`，漏掉本仓自定义出口 ⇒ 真 panic 判绿
        （evidence/70/70.12/probe-window.qemu-serial.log 物证）。裸子串
        方言偶然兜住它们，锚定词表反而漏检——收编前必须先补词表。
        fixture 每行对应一个真实 emitter（注释在 toml PANIC-SCAN 段）。
        """
        entry = get_criterion("PANIC-SCAN")
        tokens = list(entry["none_of"])
        # (fixture 行号, 该行的 emitter 形, 必须命中它的 token)
        forms = [
            (1, "root-task panic.rs:269 WDT unmapped", "PANIC #"),
            (2, "services panic.rs:50 supervisor restart", "PANIC #"),
            (3, "root-task panic.rs:257 arming watchdog reset", "PANIC reboot #"),
            (4, "runtime/hello main.rs:68 用户态组件 panic", "[PANIC]"),
        ]
        lines = (FALSE_GREEN / "panic-hash-form.log").read_text().splitlines()
        for idx, label, token in forms:
            self.assertIn(token, tokens, f"词表缺 token {token!r}（{label}）")
            hits = count_in_file(str(FALSE_GREEN / "panic-hash-form.log"), [token])
            self.assertGreaterEqual(hits, 1, f"未命中 emitter 形：{label}")
        code, msg = run_check("PANIC-SCAN", [str(FALSE_GREEN / "panic-hash-form.log")])
        self.assertEqual(code, 1, f"四行真 panic 必须整体 ✗（得到 {msg!r}）")

    def test_standard_panic_form_still_covered(self):
        """补词表后标准形不得回归（`PANIC: panicked at` 仍恰计一次）。"""
        self.assertEqual(
            count_in_file(self.good, ["PANIC: panicked at"]), 0,
            "健康窗不应出现 panic 形",
        )
        p = str(Path(self.tmp.name, "std-panic.log"))
        with open(p, "w") as fh:
            fh.write("[svc-telemetry] PANIC: panicked at src/x.rs:1:1\n")
        code, _ = run_check("PANIC-SCAN", [p])
        self.assertEqual(code, 1, "标准 panic 形必须 ✗")

    def test_live_gates_do_not_bare_grep_panic(self):
        """判据垄断守卫：活体 run70 门不得再自带裸 PANIC grep 方言。

        登记册 PANIC-SCAN 是零-PANIC 判据的唯一执行面；本守卫把"退役"从
        注释宣称变成可执行事实（73 批B 只对 窗B/B5 兑现，A6/C4/R7/vol7
        仍是裸 grep——文档与码不符即红）。run69 系已被 70 系取代且无任何
        调用方，不在守卫面内。
        """
        banned = [
            'grep -aci "PANIC',   # vol5 R7 旧 -i 宽面
            'grep -ac "PANIC\\|',  # r10-held A6/C4 第三词表
            "grep -aqcE 'PANIC",  # vol7 R1/R5/R13
            "! grep -aqcE 'PANIC",
        ]
        targets = []
        for path in sorted((ROOT / "tools").glob("run70-*.sh")):
            # 判据面 = 会打出门 verdict 行的脚本；纯人工诊断转储（无
            # `[✓][` 回声）不构成判据，不在守卫面内。
            if "[✓][" in path.read_text(errors="replace"):
                targets.append(path)
        names = {p.name for p in targets}
        # 守卫面必须由"出门即有 verdict 回声"这条规则决定，不能退回按文件名
        # glob——`run70-vol*-gate.sh` 曾漏掉下面两个真实判据脚本（本守卫的
        # 第一版就是这么漏的）。
        for required in ("run70-r03-powercut.sh", "run70-r10-held.sh"):
            self.assertIn(required, names, f"守卫面漏掉判据脚本 {required}")
        offenders = []
        for path in targets:
            if not path.is_file():
                continue
            text = path.read_text(errors="replace")
            for needle in banned:
                if needle in text:
                    offenders.append(f"{path.name}: {needle}")
        self.assertEqual(offenders, [], "裸 PANIC grep 方言仍在活体门：\n  " + "\n  ".join(offenders))

        # 结构化裁决器同样不得自带第二套词表（它曾是"第四方言"）。
        judge = (ROOT / "tools" / "run70_gate_judge.py").read_text(errors="replace")
        for needle in ('find_all("PANIC', 'find_all("memory allocation'):
            self.assertNotIn(needle, judge, f"judge.py 自带硬编码 PANIC 词表：{needle}")
        self.assertIn(
            "get_criterion(\"PANIC-SCAN\")", judge,
            "judge.py 的 G1/G2 必须读登记册 PANIC-SCAN（词表单源）",
        )

    def test_boundary_open_only_for_non_word_tail(self):
        """右边界推广到"以分隔符结尾"后，粘连/错形仍必须不命中。"""
        p = str(Path(self.tmp.name, "hash-glue.log"))
        with open(p, "w") as fh:
            fh.write(
                "SP-PANIC #1 glued left\n"      # 左边界 '-' 在类内 ⇒ 不命中
                "PANIC: panicked at ok\n"        # 标准形不是 "# " 形
                "PANICX #1 not the token\n"
            )
        self.assertEqual(count_in_file(p, ["PANIC #"]), 0, "左粘连/变形不得命中")
        with open(p, "a") as fh:
            fh.write("[svc-x] PANIC #3 at src/panic.rs:1 — suspending\n")
        self.assertEqual(count_in_file(p, ["PANIC #"]), 1, "真 # 形命中恰一次")

    def test_count_only_b4(self):
        p = str(Path(self.tmp.name, "b4.log"))
        with open(p, "w") as fh:
            fh.write("[svc-control-dispatch] EXEC-JOURNAL applied-confirmed a\n" * 3)
        code, msg = run_check("R10-B4-DELIVERY", [p], count_only=True)
        self.assertEqual((code, msg), (0, "3"))
        self.assertEqual(
            count_in_file(self.good, ["EXEC-JOURNAL applied-confirmed"]), 1
        )

    def test_acq_modbus(self):
        code, _ = run_check("ACQ-MODBUS", [self.good])
        self.assertEqual(code, 0)
        code, _ = run_check("ACQ-MODBUS", [str(FALSE_GREEN / "r9-only-boot-identity.log")])
        self.assertEqual(code, 1, "无 ACQ 行的卷宗必须 ✗")

    def test_gate_terminal_status_shared_fixture_lock(self):
        """P4：py 侧 gate_terminal_status 对共享 fixture 逐行一致（文件在场时）。

        xtask（Rust）侧在 evidence_chain 测试消费同一文件——两侧判定必须
        完全一致（C5：共享的是 fixture 数据，不是代码）。
        """
        if not TERMINAL_FIXTURES.is_file():
            self.skipTest("gate-terminal-fixtures.txt 尚未落盘（B-P4 交付）")
        import importlib.util

        spec = importlib.util.spec_from_file_location(
            "run70_candidate_evidence", TOOLS / "run70-candidate-evidence.py"
        )
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        cases = _parse_terminal_fixtures()
        self.assertGreater(len(cases), 0, "共享 fixture 不得为空")
        for gate, line, expected in cases:
            with self.subTest(gate=gate, line=line):
                got = mod.gate_terminal_status(line, gate)
                want = {"PASS": True, "FAIL": False, "NONE": None}[expected]
                self.assertEqual(got, want)


def _parse_terminal_fixtures() -> list[tuple[str, str, str]]:
    """fixture 行：<gate> '|' <样例行> '|' <期望: PASS|FAIL|NONE>（# 注释）。"""
    out = []
    for raw in TERMINAL_FIXTURES.read_text().splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        gate, sample, expected = (part.strip() for part in line.split("|"))
        out.append((gate, sample, expected))
    return out


if __name__ == "__main__":
    sys.exit(main(sys.argv))
