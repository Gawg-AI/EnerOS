#!/usr/bin/env python3
"""R10 门段A 计划守恒对账（F70-05/R5 重订：逐计划完整身份关联）。

从串口原始工件重算（蓝图 08 §288 验收纪律——不搜 PASS 词形凑数）。
F70-05 起**机器身份 = 完整 16B plan/intent id**（引擎观测行携带 plan=
全量 id；旧 4B 前缀只供阅读，不得作为唯一机器身份——前缀相同的两个
不同对象不得合并/互换账目）。对每个 PLAN-HELD 计划 P（按完整 plan id
关联）：

  - 编译行：**同 plan id** 的 PLAN-COMPILED 行（intents I + skipped S；
    同计划出现多条编译行 = 矛盾，fail-closed）；
  - HELD-PLAN-DONE plan=P：applied A + blocked Bk + failed F + state
    （同计划多条 DONE = 矛盾）；
  - `plan=P` 的 HELD-COMMAND 行：唯一完整 intent id 的 Applied 计数
    （周期重复打印不计数）；readback=Some 按 intent 去重（同一读至多
    消费一次——重复打印不加数）。

通过线（输出满足计划的七个数；逐计划独立计算，不互相补数）：
  I + S == 12 ∧ A + Bk + F == I ∧ state=Committed ∧ A == I
  ∧ 唯一 Applied intent 数 == I ∧ 该计划 readback≥1。

退出码（父 gate 检查退出码与结果——解析异常/截断/矛盾不得变成 0 计数
碰巧过门）：0 = 找到满足计划；1 = NONE（无满足计划/缺证据）；2 = 输入
不可判（无完整 id 的历史日志 / 未归属命令行 / 矛盾重复）。

行内无完整 16B id ⇒ 拒绝（历史 4B 前缀日志不能证明完整身份——按范围
标"历史/不可用于新门"，不补造 ID）。

输出：`I SK A BK F UNIQ RBS`（单行）或 `NONE`。
"""
import re
import sys

TARGET_ASSIGNMENTS = 12  # 3 资源 × 4 时段

ID_LEN = 16


def pid_hex(pid):
    """完整 id 的前 4B 阅览形（仅诊断输出用——机器身份恒为完整 id）。"""
    return bytes(pid)[:4].hex()


def parse_id(text):
    """`[0x52, 52, ...]` 形态 → 16B 元组；长度 ≠16 ⇒ None。"""
    tokens = [t.strip() for t in text.strip("[]").split(",") if t.strip()]
    try:
        vals = tuple(int(t, 16) & 0xFF for t in tokens)
    except ValueError:
        return None
    return vals if len(vals) == ID_LEN else None


class Reject(Exception):
    """输入不可判（exit 2）——不猜、不补造身份。"""


def parse_log(lines):
    held = []  # 按日志序的完整 plan id
    compiled = {}  # pid -> (intents, skipped)
    done = {}  # pid -> (applied, blocked, failed, state)
    applied_ids = {}  # pid -> set(intent_id)
    readback_ids = {}  # pid -> set(intent_id)（readback=Some，按 intent 去重）
    for lineno, l in enumerate(lines, 1):
        m = re.search(r"PLAN-HELD plan=\[([0-9a-fx, ]+)\]", l)
        if m:
            pid = parse_id(m.group(1))
            if pid is None:
                raise Reject(f"line {lineno}: PLAN-HELD 无完整 16B plan id（历史 4B 前缀日志不能作为机器身份）")
            held.append(pid)
            continue
        m = re.search(
            r"PLAN-COMPILED batches=\d+ intents=(\d+) skipped=(\d+).* plan=\[([0-9a-fx, ]+)\]",
            l,
        )
        if m:
            pid = parse_id(m.group(3))
            if pid is None:
                raise Reject(f"line {lineno}: PLAN-COMPILED 无完整 16B plan id")
            row = (int(m.group(1)), int(m.group(2)))
            if compiled.get(pid, row) != row:
                raise Reject(f"line {lineno}: plan {pid_hex(pid)} 编译行矛盾（重复且数值不同）")
            compiled[pid] = row
            continue
        m = re.search(
            r"HELD-PLAN-DONE plan=\[([0-9a-fx, ]+)\] batches=\d+"
            r" applied=(\d+) blocked=(\d+) failed=(\d+) state=(\w+)",
            l,
        )
        if m:
            pid = parse_id(m.group(1))
            if pid is None:
                raise Reject(f"line {lineno}: HELD-PLAN-DONE 无完整 16B plan id")
            row = (int(m.group(2)), int(m.group(3)), int(m.group(4)), m.group(5))
            if done.get(pid, row) != row:
                raise Reject(f"line {lineno}: plan {pid_hex(pid)} DONE 行矛盾（重复且数值不同）")
            done[pid] = row
            continue
        m = re.search(
            r"HELD-COMMAND intent=\[([0-9a-fx, ]+)\] state=(\w+)(.*)",
            l,
        )
        if m:
            iid = parse_id(m.group(1))
            # plan= 归属在行内独立检索（recheck RR2：HELD-COMMAND 行尾已
            # 追加 dev/pt/val 绑定锚——旧正则把 plan= 锚在行尾会漏归属）。
            pm = re.search(r"plan=\[([0-9a-fx, ]+)\]", m.group(3))
            pid = parse_id(pm.group(1)) if pm else None
            if iid is None or pid is None:
                # 无完整 intent id 或无 plan 归属的 Applied 行：把它计入
                # 任何计划都会重开跨计划借数（R5 原缺陷）——fail-closed。
                if m.group(2) == "Applied":
                    raise Reject(
                        f"line {lineno}: HELD-COMMAND Applied 缺完整 16B intent/plan 归属（不可归属即拒绝）"
                    )
                continue
            if m.group(2) == "Applied":
                applied_ids.setdefault(pid, set()).add(iid)
                if "readback=Some" in (m.group(3) or ""):
                    # 同一 intent 的独立读至多消费一次（重复打印不加数）。
                    readback_ids.setdefault(pid, set()).add(iid)
            continue
    return held, compiled, done, applied_ids, readback_ids


def account(pid, compiled, done, applied_ids, readback_ids):
    """对单个计划独立计算七元组；返回 dict 或 None（证据不全）。"""
    if pid not in compiled or pid not in done:
        return None
    intents, skipped = compiled[pid]
    applied, blocked, failed, state = done[pid]
    return {
        "intents": intents,
        "skipped": skipped,
        "applied": applied,
        "blocked": blocked,
        "failed": failed,
        "state": state,
        "uniq": len(applied_ids.get(pid, ())),
        "rbs": len(readback_ids.get(pid, ())),
    }


def passes(row):
    """验收线（唯一执行面）。

    消费方（run70-r10-held.sh 段A 的 A2/A3/A4a/A4c/A7）只是本谓词的**观测
    回声**，不得重算——重算在 select 已通过的分支里结构不可达，等于摆设。
    七元组每臂各自必要，selftest 有逐臂证伪表。
    """
    return (
        row["intents"] + row["skipped"] == TARGET_ASSIGNMENTS
        and row["applied"] + row["blocked"] + row["failed"] == row["intents"]
        and row["state"] == "Committed"
        and row["applied"] == row["intents"]
        and row["uniq"] == row["intents"]
        and row["rbs"] >= 1
    )


def select(lines):
    """选定满足验收线的计划：返回 (完整 plan id, 七元组) 或 None（证据
    不全/无满足计划）。选择谓词与 main 的通过线同源（recheck RR2：
    r10_intent_bind 按同一选中计划做外部事务归属）。Reject 照常上抛。"""
    held, compiled, done, applied_ids, readback_ids = parse_log(lines)
    for pid in held:
        row = account(pid, compiled, done, applied_ids, readback_ids)
        if row is None:
            continue
        if passes(row):
            return pid, row
    return None


def diagnose(lines):
    """NONE 分支的逐计划缺口说明（stderr——不静默 0 计数）。select 已
    返回 None（未 Reject）⇒ 本重解析安全。"""
    held, compiled, done, applied_ids, readback_ids = parse_log(lines)
    for pid in held:
        row = account(pid, compiled, done, applied_ids, readback_ids)
        if row is not None:
            print(
                "r10_plan_accounting: plan {} I={} S={} A={} Bk={} F={} state={} uniq={} rbs={}".format(
                    pid_hex(pid), row["intents"], row["skipped"], row["applied"],
                    row["blocked"], row["failed"], row["state"], row["uniq"], row["rbs"],
                ),
                file=sys.stderr,
            )
        else:
            print(
                f"r10_plan_accounting: plan {pid_hex(pid)} 证据不全（无编译行或无 DONE）",
                file=sys.stderr,
            )
    if not held:
        print("r10_plan_accounting: 无 PLAN-HELD 计划", file=sys.stderr)


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "--selftest":
        raise SystemExit(selftest())
    if len(sys.argv) != 2:
        print("usage: r10_plan_accounting.py <serial-log> | --selftest", file=sys.stderr)
        raise SystemExit(2)
    try:
        lines = open(sys.argv[1]).read().splitlines()
        picked = select(lines)
    except Reject as e:
        print(f"r10_plan_accounting: REJECT {e}", file=sys.stderr)
        print("NONE")
        raise SystemExit(2)
    if picked is None:
        # 说明缺口：给出逐计划账目（哪些计划差什么——不静默 0 计数）。
        diagnose(lines)
        print("NONE")
        raise SystemExit(1)
    _best_pid, best = picked
    print(
        f"{best['intents']} {best['skipped']} {best['applied']} "
        f"{best['blocked']} {best['failed']} {best['uniq']} {best['rbs']}"
    )
    raise SystemExit(0)


def _hex_id(tag, n=ID_LEN):
    return ", ".join(f"{(tag + i) & 0xFF:02x}" for i in range(n))


def selftest():
    """F70-05 A01～A06 host 判据锁：正例通过、对抗 fixture 全部拒绝。"""
    import tempfile
    import os

    failures = []

    def run(text):
        with tempfile.NamedTemporaryFile("w", suffix=".log", delete=False) as fh:
            fh.write(text)
            path = fh.name
        try:
            import subprocess

            r = subprocess.run(
                [sys.executable, __file__, path], capture_output=True, text=True
            )
            return r.returncode, r.stdout.strip(), r.stderr.strip()
        finally:
            os.unlink(path)

    def check(label, cond, detail=""):
        print(f"[selftest] {'PASS' if cond else 'FAIL'} {label}{'' if cond else ': ' + detail}")
        if not cond:
            failures.append(label)

    A = _hex_id(0xA1)
    B = _hex_id(0xB1)

    def cmd(pid, iid, state="Applied", readback=True, dev=1, pt=10, val=1.0):
        rb = "readback=Some(1.05)" if readback else "readback=None"
        # recheck RR2：行尾追加 dev/pt/val 绑定锚（本脚本只消费 intent/plan
        # 归属——锁住「新行形不破坏守恒解析」）。
        return (
            f"[svc-control-dispatch] HELD-COMMAND intent=[{iid}] state={state} "
            f"attempts=1 {rb} reason=- plan=[{pid}] dev=0x{dev:x} pt={pt} val={val}"
        )

    def compiled(pid, i, s):
        return (
            f"[svc-control-dispatch] PLAN-COMPILED batches=3 intents={i} skipped={s} manual=0 "
            f"compiler=[11, 22, 33, 44] batch=[55, 66, 77, 88] plan=[{pid}]"
        )

    def heldline(pid):
        return f"[svc-control-dispatch] PLAN-HELD plan=[{pid}] batches=3 intents=6"

    def doneline(pid, a, bk, f, state="Committed"):
        return (
            f"[svc-control-dispatch] HELD-PLAN-DONE plan=[{pid}] batches=3 "
            f"applied={a} blocked={bk} failed={f} state={state}"
        )

    # A01：单计划完整编译、唯一 intent 终态、实际读回 → 通过。
    ids = [_hex_id(0x10 + k) for k in range(6)]
    log_a01 = "\n".join(
        [compiled(A, 6, 6), heldline(A)]
        + [cmd(A, i) for i in ids]
        + [doneline(A, 6, 0, 0)]
    )
    rc, out, err = run(log_a01)
    check("A01 完整正例通过", rc == 0 and out == "6 6 6 0 0 6 6", f"rc={rc} out={out!r} err={err!r}")

    # A02：P 只有自报 DONE、全部 Applied 属于 Q → P 必须失败（R5 核心）。
    log_a02 = "\n".join(
        [compiled(A, 6, 6), heldline(A), doneline(A, 6, 0, 0), heldline(B), doneline(B, 6, 0, 0)]
        + [cmd(B, i) for i in ids]
    )
    # B 无编译行 → 证据不全；A 无自己的 Applied → uniq=0。
    rc, out, err = run(log_a02)
    check("A02 跨计划借数被拒", rc == 1 and out == "NONE", f"rc={rc} out={out!r}")
    # 补上 B 的编译行：B 靠自己的证据通过；A 仍不得通过。
    log_a02b = "\n".join(
        [compiled(A, 6, 6), compiled(B, 6, 6), heldline(A), heldline(B)]
        + [cmd(B, i) for i in ids]
        + [doneline(A, 6, 0, 0), doneline(B, 6, 0, 0)]
    )
    rc, out, err = run(log_a02b)
    check("A02b 各自独立计算（B 过、A 的 uniq=0 不借 B）", rc == 0 and out == "6 6 6 0 0 6 6", f"rc={rc} out={out!r}")

    # A03：前 4B 相同、完整 ID 不同；重复打印不加数。
    a_same = ", ".join(["aa"] * 4 + ["01"] * 12)
    b_same = ", ".join(["aa"] * 4 + ["02"] * 12)
    ids2 = [", ".join(["c0"] * 15 + [f"{k:02x}"]) for k in range(6)]
    log_a03 = "\n".join(
        [
            compiled(a_same, 6, 6),
            compiled(b_same, 6, 6),
            heldline(a_same),
            heldline(b_same),
        ]
        + [cmd(a_same, i) for i in ids2]
        + [cmd(a_same, i) for i in ids2]  # 周期重复打印 ×2 → 不加数
        + [doneline(a_same, 6, 0, 0), doneline(b_same, 6, 0, 0)]
    )
    rc, out, err = run(log_a03)
    check(
        "A03 前缀碰撞分账 + 重复打印不加数",
        rc == 0 and out == "6 6 6 0 0 6 6",
        f"rc={rc} out={out!r} err={err!r}",
    )

    # A04：P 的 Applied 齐全，但 readback 全部属于 Q → P 的 rbs=0 ⇒ 失败。
    log_a04 = "\n".join(
        [compiled(A, 6, 6), heldline(A), heldline(B), compiled(B, 6, 6)]
        + [cmd(A, i, readback=False) for i in ids]
        + [cmd(B, i) for i in ids]
        + [doneline(A, 6, 0, 0), doneline(B, 6, 0, 0)]
    )
    rc, out, err = run(log_a04)
    check("A04 读回跨计划不补数（A 的 rbs=0 拒绝，B 通过）", rc == 0 and out == "6 6 6 0 0 6 6", f"rc={rc} out={out!r}")
    log_a04b = "\n".join(
        [compiled(A, 6, 6), heldline(A)]
        + [cmd(A, i, readback=False) for i in ids]
        + [doneline(A, 6, 0, 0)]
    )
    rc, out, err = run(log_a04b)
    check("A04b 零读回计划拒绝", rc == 1 and out == "NONE", f"rc={rc} out={out!r}")

    # A05：同一 intent 的 readback 重复打印至多消费一次。
    log_a05 = "\n".join(
        [compiled(A, 6, 6), heldline(A)]
        + [cmd(A, ids[0]), cmd(A, ids[0]), cmd(A, ids[0])]
        + [cmd(A, i) for i in ids[1:]]
        + [doneline(A, 6, 0, 0)]
    )
    rc, out, err = run(log_a05)
    check("A05 读回按 intent 去重", rc == 0 and out.split()[-1] == "6", f"rc={rc} out={out!r}")

    # A06a：DONE 计数矛盾（A+Bk+F ≠ I）→ 失败并说明缺口。
    log_a06a = "\n".join(
        [compiled(A, 6, 6), heldline(A)] + [cmd(A, i) for i in ids] + [doneline(A, 5, 1, 0)]
    )
    rc, out, err = run(log_a06a)
    check("A06a 终态守恒矛盾拒绝", rc == 1 and out == "NONE" and "A=5" in err, f"rc={rc} err={err!r}")

    # A06b：日志截断（有 HELD 无 DONE）→ 失败。
    log_a06b = "\n".join([compiled(A, 6, 6), heldline(A)] + [cmd(A, i) for i in ids])
    rc, out, err = run(log_a06b)
    check("A06b 截断（无 DONE）拒绝", rc == 1 and out == "NONE", f"rc={rc}")

    # A06c：同一计划重复矛盾编译行 → exit 2。
    log_a06c = "\n".join(
        [compiled(A, 6, 6), compiled(A, 5, 7), heldline(A), doneline(A, 6, 0, 0)]
        + [cmd(A, i) for i in ids]
    )
    rc, out, err = run(log_a06c)
    check("A06c 编译行矛盾 exit 2", rc == 2, f"rc={rc}")

    # A06d：无完整 id 的历史形态（4B 前缀）→ 拒绝（不补造身份）。
    short = "aa, bb, cc, dd"
    log_a06d = "\n".join(
        [
            f"[svc-control-dispatch] PLAN-COMPILED batches=3 intents=6 skipped=6 manual=0 compiler=[11, 22] batch=[33] plan=[{short}]",
            f"[svc-control-dispatch] PLAN-HELD plan=[{short}] batches=3 intents=6",
            f"[svc-control-dispatch] HELD-PLAN-DONE plan=[{short}] batches=3 applied=6 blocked=0 failed=0 state=Committed",
        ]
    )
    rc, out, err = run(log_a06d)
    check("A06d 4B 前缀历史形态拒绝", rc == 2, f"rc={rc} err={err!r}")

    # A06e：Applied 行缺 plan 归属 → 拒绝（不可归属即不可计数）。
    orphan = (
        f"[svc-control-dispatch] HELD-COMMAND intent=[{ids[0]}] state=Applied "
        "attempts=1 readback=Some(1.0) reason=-"
    )
    log_a06e = "\n".join(
        [compiled(A, 6, 6), heldline(A), orphan] + [cmd(A, i) for i in ids[1:]] + [doneline(A, 6, 0, 0)]
    )
    rc, out, err = run(log_a06e)
    check("A06e 未归属 Applied 行拒绝", rc == 2, f"rc={rc} err={err!r}")

    # A07（73 批C）：通过线的**逐臂证伪表**。run70-r10-held 段A 原先在
    # bash 里重算 I+S=12 / uniq≥I / rbs≥1 / A+Bk+F=I——那些臂位于
    # select 已通过的分支内，结构不可达（等于摆设）。本表是各臂唯一的
    # 独立覆盖：任一臂失效即红。
    def row(**kw):
        base = dict(intents=6, skipped=6, applied=6, blocked=0, failed=0,
                    state="Committed", uniq=6, rbs=1)
        base.update(kw)
        return base

    check("A07 基线七元组通过", passes(row()))
    for arm, bad in {
        "I+S==12": row(skipped=5),
        "A+Bk+F==I": row(blocked=1),
        "state==Committed": row(state="Partial"),
        "A==I": row(applied=5, failed=1),
        "uniq==I": row(uniq=5),
        "rbs>=1": row(rbs=0),
    }.items():
        check(f"A07 臂必要：破坏 {arm} 即拒", not passes(bad), f"{arm} 破坏仍通过=该臂失效")

    # A07b：intents=0 的计划必须被拒（可构造形态，非合成七元组）——
    # Applied+readback 令 uniq=1 撞 uniq==intents 臂。旧 bash A4a 的
    # "I>=1" 由此臂**蕴含**，不是独立判据。
    log_a07 = "\n".join(
        [compiled(A, 0, 12), heldline(A), cmd(A, ids[0]), doneline(A, 0, 0, 0)]
    )
    rc, out, err = run(log_a07)
    check("A07b intents=0 带 Applied 读回 → uniq 臂拒绝",
          rc == 1 and out == "NONE", f"rc={rc} out={out!r} err={err!r}")

    if failures:
        print(f"[selftest] {len(failures)} FAILED")
        return 1
    print("[selftest] all PASS")
    return 0


if __name__ == "__main__":
    main()
