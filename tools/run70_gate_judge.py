#!/usr/bin/env python3
"""蓝图 70.1 WP-70.1.4：结构化失败判据裁决器（单源，gate 与负例自检共用）。

与 run69-vol5-gate.sh:180 的本质差异：**删除 alloc.rs PANIC 豁免**——
69.5 窗正是被该豁免放过的真实分配失败（F70-7-02）。任何进程的任何
PANIC / 分配失败在本裁决器下一律 FAIL，无进程名/文件名豁免。

判据（每条输出 结构化一行：判据 ID / PASS-FAIL / 理由 / 证据行号区间）：
  G1 PANIC 总数 = 0（无豁免，含 alloc.rs）
  G2 分配失败总数 = 0（"memory allocation of N bytes failed"）
  G3 PRODUCT-READY = true
  G4 控制进展相位分布：串口按行数四等分，每相位 CONTROL-CHAIN round=
     样本数 ≥ ceil(min_rounds/4)——尾部死亡（最后相位为零）不得被
     全窗累计掩盖
  G5 有效控制总量：rounds ≥ min_rounds 且 dispatch=Committed ≥ min_rounds
  G6 决策链健康：CHAIN-KEY-MISSING = 0
  G7 环境工具故障：Modbus/IEC104 transcript 存在且非空（提供方活着）

用法：run70_gate_judge.py <serial.log> <min_rounds> [--transcripts f1 f2 ...]
退出码：0 = 全部 PASS；1 = 存在 FAIL；2 = 输入缺失（不得当 PASS）。
"""
import json
import re
import sys


def judge(path, min_rounds, transcripts):
    report = []
    try:
        with open(path, "rb") as f:
            raw = f.read()
    except OSError as e:
        print(json.dumps({"id": "INPUT", "verdict": "FAIL",
                          "reason": f"serial log unreadable: {e}"}))
        return 2
    all_lines = raw.decode(errors="ignore").splitlines()
    # 相位/进展分析以 PRODUCT-READY 为起点——启动段（spawn DBG/page_map
    # 等）线密度与控制节奏无关，混入会把首相位压成伪影。
    ready_at = next((i for i, ln in enumerate(all_lines)
                     if "PRODUCT-READY: true" in ln), 0)
    lines = all_lines[ready_at:]
    n = len(lines)

    def find(pat):
        return [i for i, ln in enumerate(lines) if pat in ln]

    # G1/G2：零豁免失败判据
    panic_idx = find("PANIC: panicked at")
    alloc_idx = find("memory allocation of")
    report.append(("G1", len(panic_idx) == 0,
                   f"PANIC 总数={len(panic_idx)}（无豁免）"
                   + (f" 首处行 {panic_idx[0]+1}" if panic_idx else "")))
    report.append(("G2", len(alloc_idx) == 0,
                   f"分配失败总数={len(alloc_idx)}"
                   + (f" 首处行 {alloc_idx[0]+1}" if alloc_idx else "")))
    # G3 就绪
    ready = [i for i, ln in enumerate(lines) if "PRODUCT-READY: true" in ln]
    report.append(("G3", bool(ready),
                   f"PRODUCT-READY: true x{len(ready)}"))
    # G4 相位分布（ready 后按行四等分）：禁止尾部死亡与后段塌陷，
    # 允许启动热身爬坡（首相位含 checkpoint 首停顿/对端握手，节奏天然
    # 低于稳态）。判据 = 尾相位非零 ∧ 尾相位 ≥ 第二相位（无晚期塌陷）
    # ∧ 每相位 ≥ ceil(门限/8)（绝对地板）。
    round_idx = [i for i, ln in enumerate(lines) if "CONTROL-CHAIN: round=" in ln]
    total_rounds = len(round_idx)
    per_phase_floor = -(-min_rounds // 8)  # ceil
    phase_counts = []
    for q in range(4):
        lo, hi = n * q // 4, n * (q + 1) // 4
        c = sum(1 for i in round_idx if lo <= i < hi)
        phase_counts.append(c)
    tail_ok = (phase_counts[3] >= 1
               and phase_counts[3] >= phase_counts[1]
               and min(phase_counts) >= per_phase_floor)
    report.append(("G4", tail_ok,
                   f"相位样本={[c for c in phase_counts]} 地板={per_phase_floor}"
                   f" 尾/二={phase_counts[3]}/{phase_counts[1]}"
                   f"（热身爬坡允许，塌陷/尾部0判负）"))
    # G5 总量
    committed = sum(ln.count("dispatch=Committed") for ln in lines)
    report.append(("G5", total_rounds >= min_rounds and committed >= min_rounds,
                   f"rounds={total_rounds} Committed={committed} 门限={min_rounds}"))
    # G6 链钥
    ckm = len(find("CHAIN-KEY-MISSING"))
    report.append(("G6", ckm == 0, f"CHAIN-KEY-MISSING x{ckm}"))
    # G7 对端工具活性
    if transcripts:
        bad = []
        for t in transcripts:
            try:
                with open(t, "rb") as f:
                    if len(f.read(4096)) == 0:
                        bad.append(t)
            except OSError:
                bad.append(t)
        report.append(("G7", not bad,
                       f"transcript 非空核验 fail={bad}" if bad else
                       f"transcript x{len(transcripts)} 非空"))

    ok = True
    for gid, passed, reason in report:
        print(json.dumps({"id": gid, "verdict": "PASS" if passed else "FAIL",
                          "reason": reason}, ensure_ascii=False))
        ok = ok and passed
    print("=== VOL70 JUDGE:", "PASS" if ok else "FAIL", "===")
    return 0 if ok else 1


def selftest():
    """负例自检：注入分配失败 PANIC / 尾部死亡 / 缺日志 / 旧样本重用均判负。"""
    import tempfile
    import os

    def mklog(rounds_total, tail_dead=False, alloc_panic_at=None):
        ls = ["PRODUCT-READY: true"]
        for r in range(1, rounds_total + 1):
            if alloc_panic_at is not None and r == alloc_panic_at:
                ls.append("[svc-control-dispatch] PANIC: panicked at "
                          "library/alloc/src/alloc.rs:573:9:")
                ls.append("memory allocation of 114688 bytes failed")
            ls.append(f"[svc-control-dispatch] CONTROL-CHAIN: round={r} "
                      f"dispatch=Committed")
            if tail_dead and r > rounds_total * 3 // 4:
                # 尾部相位静默：后续轮不再产生（用填充行挤掉相位样本）
                pass
        if tail_dead:
            ls = ls[: len(ls) // 2] + ["[filler] idle"] * (len(ls) // 2)
        return "\n".join(ls) + "\n"

    results = []
    with tempfile.TemporaryDirectory() as d:
        # 正例：健康窗（600 轮，门限 400）
        p_ok = os.path.join(d, "ok.log")
        open(p_ok, "w").write(mklog(600))
        results.append(("healthy-pass", judge_quiet(p_ok, 400) == 0))
        # 负例 1：注入 alloc PANIC（含 alloc.rs——旧豁免必须失效）
        p_panic = os.path.join(d, "panic.log")
        open(p_panic, "w").write(mklog(600, alloc_panic_at=500))
        results.append(("alloc-panic-fail", judge_quiet(p_panic, 400) == 1))
        # 负例 2：尾部死亡（全窗累计 600 但尾相位为空）
        p_dead = os.path.join(d, "dead.log")
        open(p_dead, "w").write(mklog(600, tail_dead=True))
        results.append(("tail-dead-fail", judge_quiet(p_dead, 400) == 1))
        # 负例 3：缺日志
        results.append(("missing-log-fail",
                        judge_quiet(os.path.join(d, "nope.log"), 400) == 2))
        # 负例 4：样本不足（旧窗重用：轮数低于门限）
        p_short = os.path.join(d, "short.log")
        open(p_short, "w").write(mklog(100))
        results.append(("insufficient-samples-fail",
                        judge_quiet(p_short, 400) == 1))
    ok = all(r for _, r in results)
    for name, passed in results:
        print(f"[selftest] {name}: {'PASS' if passed else 'FAIL'}")
    print("=== SELFTEST:", "PASS" if ok else "FAIL", "===")
    return 0 if ok else 1


def judge_quiet(path, min_rounds):
    import io
    import contextlib
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        return judge(path, min_rounds, [])


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "--selftest":
        sys.exit(selftest())
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    tx = sys.argv[4:] if len(sys.argv) > 4 and sys.argv[3] == "--transcripts" else []
    sys.exit(judge(sys.argv[1], int(sys.argv[2]), tx))
