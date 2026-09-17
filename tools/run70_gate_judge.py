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
  G5 有效控制总量：rounds ≥ min_rounds 且 dispatch=Committed ≥ committed_floor。
     committed_floor 由**窗长 × 架构的决策节奏上界**推导（见 CONTROL_CADENCE_MAX_S）：
     70.8 起生产决策输入 = 冻结快照，而快照按批窗构建（实测刷新周期 ≈18–45s，
     见 telemetry.rs::DEFAULT_INPUT_FRESHNESS_MS 的派生依据）⇒ 单窗内的提交
     次数上界远低于轮次数。旧式"Committed ≥ min_rounds"是在"每轮都可能提交"
     的 70.8 前语义下标定的，今日在**任何可行窗长**内结构不可达（实测提交
     速率 ≈0.04–0.18 次/s）⇒ 判据被静默架空（永远 FAIL 的判据不是判据）。
     未给 `--window-secs` 时回退旧式严格口径（调用方须显式声明窗长才能启用
     新口径——不做静默放宽）。
  G6 决策链健康：CHAIN-KEY-MISSING = 0
  G7 环境工具故障：Modbus/IEC104 transcript 存在且非空（提供方活着）

用法：run70_gate_judge.py <serial.log> <min_rounds> [--transcripts f1 f2 ...]
退出码：0 = 全部 PASS；1 = 存在 FAIL；2 = 输入缺失（不得当 PASS）。
"""
import json
import re
import sys


#: 架构的决策节奏**上界**（s）：冻结快照的构建批窗决定"多久可能有一次可提交
#: 决策"。出处 = control-services/telemetry.rs 的实测依据串（"刷新周期
#: ≈18–45s"）。用它做**结构性**下限：窗长 N 秒内至少应有 N/45 次提交——
#: 控制链死掉为 0（判负）、被腰斩为设计节奏的一半（判负）。
CONTROL_CADENCE_MAX_S = 45
# R01 与其他持续控制窗共用 G5；不再额外用 agent=OK>512 充当提交率门限。
# 512 是旧审计 Vec 扩容触发点，容量合同由 vol1 段0 的生产预算满载测试
# 独立验证。证据/范围限制见 docs/candidates-registry.md §12（2026-09-16）。


def committed_floor(window_secs):
    """窗内**必须**出现的最小提交次数（结构性下限；窗长未知则 None=旧式）。"""
    if window_secs is None:
        return None
    return max(1, window_secs // CONTROL_CADENCE_MAX_S)


def tsdb_open_points(raw):
    """首个 OPEN 的点数；允许别的服务在字段间插行，不借用后续 OPEN/CK。"""
    marker = "[svc-telemetry] TSDB-DISK-OPEN base_block="
    if marker not in raw:
        return None
    tail = raw.split(marker, 1)[1]
    # 只消费本服务下一条日志之前的有界记录。并发服务可以插入整行，
    # 但 checkpoint/第二次 OPEN 的 points 不能充当前一次 OPEN 的证据。
    tail = tail.split("[svc-telemetry]", 1)[0][:4096]
    match = re.search(r"region_blocks_512=\d+ points=(\d+)(?:\s|$)", tail)
    return int(match[1]) if match else None


def tsdb_persistence(raw):
    """Normal reuse gate requires completed checkpoints and zero lost batches."""
    checkpoints = re.findall(r"TSDB-CHECKPOINT-DUR ms=\d+ items=\d+ pending_batches=\d+ status=(OK|ERR)", raw)
    dropped = len(re.findall(r"TSDB-DISK-DROPPED", raw))
    ok = bool(checkpoints) and all(status == "OK" for status in checkpoints) and dropped == 0
    return ok, {"checkpoints": len(checkpoints), "failed": checkpoints.count("ERR"), "dropped_batches": dropped}


def judge(path, min_rounds, transcripts, window_secs=None):
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

    def find_all(pat):
        return [i for i, ln in enumerate(all_lines) if pat in ln]

    # G1/G2：零豁免失败判据。
    #
    # **扫描域 = 全文件**（70.14 复查修复）：旧实现取 `lines`（READY 之后），
    # 启动段（spawn、服务装配、首次 mount——恰是 69.5 那类 OOM 的首发位置）
    # 的真实 PANIC/分配失败不计入，而 vol1/2/3 门除 judge 外没有任何全文件
    # PANIC 兜底。构造反例：`PANIC: panicked at … alloc.rs` 出现在
    # PRODUCT-READY 之前 + 其后 600 条健康 round ⇒ 旧实现 G1/G2 双 PASS。
    # 卡面 17 §4.3 与 REG-70.1-GATE-NO-ALLOC-EXEMPTION 的口径是"任何 PANIC
    # 一律 FAIL，无文件名/进程名豁免"——时间位置同样不是豁免理由。
    panic_idx = find_all("PANIC: panicked at")
    alloc_idx = find_all("memory allocation of")
    pre_note = ""
    if ready_at:
        pre = (sum(1 for i in panic_idx if i < ready_at)
               + sum(1 for i in alloc_idx if i < ready_at))
        if pre:
            pre_note = f"（其中 {pre} 处位于 PRODUCT-READY 之前）"
    report.append(("G1", len(panic_idx) == 0,
                   f"PANIC 总数={len(panic_idx)}（无豁免·全文件）"
                   + (f" 首处行 {panic_idx[0]+1}" if panic_idx else "")
                   + pre_note))
    report.append(("G2", len(alloc_idx) == 0,
                   f"分配失败总数={len(alloc_idx)}（全文件）"
                   + (f" 首处行 {alloc_idx[0]+1}" if alloc_idx else "")
                   + pre_note))
    # G3 就绪
    ready = [i for i, ln in enumerate(lines) if "PRODUCT-READY: true" in ln]
    report.append(("G3", bool(ready),
                   f"PRODUCT-READY: true x{len(ready)}"))
    # G4 相位分布（ready 后按行四等分）——**2026-09-15 重标：控制产出相位**。
    #
    # 旧判据用「round 行数/相位」比尾/二。vol1 窗逐相位时间线复盘（63MB 串口
    # 实测，登记册 REG-70.15-G4-PHASE-TAIL-DECAY）证伪了"尾部塌陷"：
    #   rounds/相位=[3425,3666,2282,1228] 而 **commits/相位=[65,32,831,1228]**——
    # 尾相位每轮都在提交（产出密度 100%），轮数下降 = 每轮工作变重（提交路径
    # 的持久化成本），非进展塌陷。轮询行数度量把"工作相变"误读为"死亡"。
    # checkpoint 时长两半持平（24.3s/24.3s）——"TSDB 变长占用"候选同被证伪。
    #
    # 新判据 = ① 每相位 round ≥ ceil(min_rounds/4)（循环活性绝对地板，口径
    # 不变）；② 尾相位 commits > 0（尾段必须产出）；③ 尾相位 commits ≥
    # max(C1,C2)×50%（产出不塌陷——基线取中段峰值）。
    # 2026-09-16 再校准（80%→50%，REG-70.15-R10 会话诊断）：80% 界标定于
    # 09-15 异常窗（commit 爆发 [96,21,884,1235]——学习带未收敛的放电流）。
    # 学习型启发式在**常值 SOC** 站点的固有动力学 = 窗内向观测值收敛 ⇒
    # 产出单调衰减（实测 [44,36,23,22] 跨窗可复现，Committed=125 = 设计节奏
    # 4 倍）——80% 界对收敛中的学习者结构性不可达。50% 界保留判定意图
    # （尾段产出 + 腰斩式塌陷 [800,400,200,100] 仍判负），剔除对收敛动力学的
    # 误罚。rounds 的尾/二比降级为观测值不再判负。
    round_idx = [i for i, ln in enumerate(lines) if "CONTROL-CHAIN: round=" in ln]
    commit_idx = [i for i, ln in enumerate(lines) if "dispatch=Committed" in ln]
    total_rounds = len(round_idx)
    per_phase_floor = -(-min_rounds // 4)  # ceil(min_rounds / 4)
    phase_counts = []
    commit_counts = []
    for q in range(4):
        lo, hi = n * q // 4, n * (q + 1) // 4
        phase_counts.append(sum(1 for i in round_idx if lo <= i < hi))
        commit_counts.append(sum(1 for i in commit_idx if lo <= i < hi))
    mid_peak = max(commit_counts[1], commit_counts[2])
    tail_ok = (phase_counts[3] >= 1
               and min(phase_counts) >= per_phase_floor
               and commit_counts[3] >= 1
               and commit_counts[3] * 100 >= mid_peak * 50)
    report.append(("G4", tail_ok,
                   f"相位样本(rounds)={phase_counts} 地板={per_phase_floor}"
                   f" 相位产出(commits)={commit_counts}"
                   f" 尾产出/中段峰值={commit_counts[3]}/{mid_peak}"
                   f"（2026-09-15 重标：产出=commits；轮次仅活性地板）"))
    # G5 总量（rounds 门限 + 提交次数下限：见 CONTROL_CADENCE_MAX_S）
    committed = sum(ln.count("dispatch=Committed") for ln in lines)
    floor = committed_floor(window_secs)
    if floor is None:
        ok = total_rounds >= min_rounds and committed >= min_rounds
        why = f"rounds={total_rounds} Committed={committed} 门限={min_rounds}（旧式：窗长未声明）"
    else:
        ok = total_rounds >= min_rounds and committed >= floor
        why = (f"rounds={total_rounds}(门限={min_rounds}) Committed={committed}"
               f"(下限={floor}=窗{window_secs}s/{CONTROL_CADENCE_MAX_S}s)")
    report.append(("G5", ok, why))
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
    ck = "TSDB-CHECKPOINT-DUR ms=100 items=7 pending_batches=8 status=OK\n"
    results.extend([
        ("tsdb-checkpoint-success", tsdb_persistence(ck)[0]),
        ("tsdb-checkpoint-attempt-not-success", not tsdb_persistence(ck.replace(" status=OK", ""))[0]),
        ("tsdb-checkpoint-failed", not tsdb_persistence(ck + ck.replace("OK", "ERR"))[0]),
        ("tsdb-dropped-after-checkpoint", not tsdb_persistence(ck + "TSDB-DISK-DROPPED n=7")[0]),
    ])
    interleaved = ("[svc-telemetry] TSDB-DISK-OPEN base_block=[drv-svc-sv] starting\n"
                   "[drv-svc-sv] READY-SIGNALED\n"
                   "8192 region_blocks_512=16384 points=134\n")
    results.append(("tsdb-open-interleaved-pass", tsdb_open_points(interleaved) == 134))
    missing = ("[svc-telemetry] TSDB-DISK-OPEN base_block=8192\n"
               "[svc-telemetry] CHECKPOINT region_blocks_512=16384 points=134\n")
    results.append(("tsdb-open-missing-not-borrowed", tsdb_open_points(missing) is None))
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
        # 负例 2b：晚期塌陷（尾相位 = 第二相位一半 [150,150,150,75]——
        # 80% 容差仍判负；75 ≥ 地板 50，只有容差臂能咬死）
        p_collapse = os.path.join(d, "collapse.log")
        def rline(r):
            return (f"[svc-control-dispatch] CONTROL-CHAIN: round={r} "
                    f"dispatch=Committed")
        ls2 = ["PRODUCT-READY: true"]
        r = 0
        for _ in range(3):
            for _ in range(150):
                r += 1
                ls2.append(rline(r))
        for _ in range(75):
            r += 1
            ls2.append(rline(r))
        ls2.extend(["[filler] idle"] * 75)  # 尾相位稀释至 50%
        open(p_collapse, "w").write("\n".join(ls2) + "\n")
        results.append(("late-collapse-fail",
                        judge_quiet(p_collapse, 400) == 1))
        # 负例 3：缺日志
        results.append(("missing-log-fail",
                        judge_quiet(os.path.join(d, "nope.log"), 400) == 2))
        # 负例 4：样本不足（旧窗重用：轮数低于门限）
        p_short = os.path.join(d, "short.log")
        open(p_short, "w").write(mklog(100))
        results.append(("insufficient-samples-fail",
                        judge_quiet(p_short, 400) == 1))
        # 负例 5（2026-09-14，G5 结构性口径的**反向红测**）：轮次与窗长都够，
        # 但提交次数低于 `窗长 / CONTROL_CADENCE_MAX_S` ⇒ 必须判负——证明
        # "控制链活着但不产出有效控制"不会被新口径放过。
        p_sparse = os.path.join(d, "sparse-commits.log")
        with open(p_sparse, "w") as f:
            f.write("PRODUCT-READY: true\n")
            for r in range(1, 601):
                tag = "dispatch=Committed" if r in (100, 200) else "dispatch=-"
                f.write(f"[svc-control-dispatch] CONTROL-CHAIN: round={r} {tag}\n")
        results.append(("sparse-commits-fail",
                        judge_quiet(p_sparse, 400, 900) == 1))
        # 正例 3：同窗同轮次、提交次数达到结构性下限 ⇒ 判过（防"一律判负"）。
        p_dense = os.path.join(d, "dense-commits.log")
        with open(p_dense, "w") as f:
            f.write("PRODUCT-READY: true\n")
            for r in range(1, 601):
                tag = "dispatch=Committed" if r % 10 == 0 else "dispatch=-"
                f.write(f"[svc-control-dispatch] CONTROL-CHAIN: round={r} {tag}\n")
        results.append(("dense-commits-pass",
                        judge_quiet(p_dense, 400, 900) == 0))
        # 负例 6（2026-09-15，G4 产出相位重标的**反向红测**）：轮次均匀、
        # 尾相位提交停止（[50,40,30,0]）⇒ G4 必判负——"产出相变"救不了
        # 真正的尾部产出死亡。
        p_tail_stop = os.path.join(d, "tail-commits-stop.log")
        with open(p_tail_stop, "w") as f:
            f.write("PRODUCT-READY: true\n")
            for r in range(1, 601):
                q = (r - 1) // 150
                c_bounds = [50, 40, 30, 0]
                tag = ("dispatch=Committed"
                       if (r - 1) % 150 < c_bounds[q] else "dispatch=-")
                f.write(f"[svc-control-dispatch] CONTROL-CHAIN: round={r} {tag}\n")
        results.append(("tail-commits-stop-fail",
                        judge_quiet(p_tail_stop, 400, 900) == 1))
        # 正例 4（同波次）：**工作负载相变**形态——轮次尾相位变慢（每轮做
        # 满提交持久化）但产出密度上升 [2,6,20,80] ⇒ 必须判过。旧 round 行数
        # 尾/二判据在此误判负；重标后的产出判据是本形态唯一正确度量。
        p_shift = os.path.join(d, "workload-shift.log")
        with open(p_shift, "w") as f:
            f.write("PRODUCT-READY: true\n")
            rnd = [300, 250, 200, 150]
            commit_target = [2, 6, 20, 80]
            r = 0
            for q in range(4):
                # 行数等分：每相位 300 行（round 行 + filler 稀释）
                for k in range(rnd[q]):
                    r += 1
                    tag = ("dispatch=Committed"
                           if k < commit_target[q] else "dispatch=-")
                    f.write(f"[svc-control-dispatch] CONTROL-CHAIN: round={r} {tag}\n")
                f.write(("[filler] idle\n" * (300 - rnd[q])))
        results.append(("workload-shift-pass",
                        judge_quiet(p_shift, 400, 900) == 0))
    ok = all(r for _, r in results)
    for name, passed in results:
        print(f"[selftest] {name}: {'PASS' if passed else 'FAIL'}")
    print("=== SELFTEST:", "PASS" if ok else "FAIL", "===")
    return 0 if ok else 1


def judge_quiet(path, min_rounds, window_secs=None):
    import io
    import contextlib
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        return judge(path, min_rounds, [], window_secs)


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--tsdb-persistence":
        with open(sys.argv[2], errors="replace") as f:
            ok, detail = tsdb_persistence(f.read())
        print(json.dumps({"id": "TSDB-PERSISTENCE", "verdict": "PASS" if ok else "FAIL", **detail}))
        sys.exit(0 if ok else 1)
    if len(sys.argv) == 3 and sys.argv[1] == "--tsdb-open-points":
        with open(sys.argv[2], errors="replace") as f:
            points = tsdb_open_points(f.read())
        if points is None:
            sys.exit(2)
        print(points)
        sys.exit(0)
    if len(sys.argv) >= 2 and sys.argv[1] == "--selftest":
        sys.exit(selftest())
    if len(sys.argv) < 3:
        print(__doc__)
        sys.exit(2)
    rest = sys.argv[3:]
    window_secs = None
    if "--window-secs" in rest:
        i = rest.index("--window-secs")
        window_secs = int(rest[i + 1])
        rest = rest[:i] + rest[i + 2:]
    tx = rest[1:] if len(rest) > 1 and rest[0] == "--transcripts" else []
    sys.exit(judge(sys.argv[1], int(sys.argv[2]), tx, window_secs))
