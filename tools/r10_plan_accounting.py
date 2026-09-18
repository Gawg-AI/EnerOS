#!/usr/bin/env python3
"""R10 门段A 计划守恒对账（2026-09-18 三资源重订）。

从串口原始工件重算（蓝图 08 §288 验收纪律——不搜 PASS 词形凑数）：
对每个 PLAN-HELD 计划（按 plan id 关联）：
  - 紧邻其前的 PLAN-COMPILED 行：intents I + skipped S（分辨率合并计入
    skipped，不丢账）；
  - HELD-PLAN-DONE plan=X：applied A + blocked Bk + failed F + state；
  - 唯一 intent id 的 HELD-COMMAND Applied 行数（周期重复打印不计数）；
  - Applied 行 readback=Some 计数（dispatch 独立读回上浮 Saga）。

通过线（输出该计划的七个数；无 ⇒ NONE）：
  I + S == 12 ∧ A + Bk + F == I ∧ state=Committed ∧ A == I。

输出：`I SK A BK F UNIQ RBS`（单行）或 `NONE`。
"""
import re
import sys

TARGET_ASSIGNMENTS = 12  # 3 资源 × 4 时段


def main():
    lines = open(sys.argv[1]).read().splitlines()
    held = []  # (line_idx, plan_hex)
    compiled = []  # (line_idx, intents, skipped)
    done = {}  # plan_hex -> (applied, blocked, failed, state)
    applied_ids = set()
    readback_some = 0
    for i, l in enumerate(lines):
        m = re.search(r"PLAN-HELD plan=\[([0-9a-fx, ]+)\]", l)
        if m:
            held.append((i, m.group(1)))
            continue
        m = re.search(r"PLAN-COMPILED batches=\d+ intents=(\d+) skipped=(\d+)", l)
        if m:
            compiled.append((i, int(m.group(1)), int(m.group(2))))
            continue
        m = re.search(
            r"HELD-PLAN-DONE plan=\[([0-9a-fx, ]+)\] batches=\d+"
            r" applied=(\d+) blocked=(\d+) failed=(\d+) state=(\w+)",
            l,
        )
        if m:
            done[m.group(1)] = (
                int(m.group(2)),
                int(m.group(3)),
                int(m.group(4)),
                m.group(5),
            )
            continue
        m = re.search(
            r"HELD-COMMAND intent=\[([0-9a-fx, ]+)\] state=Applied" r"(.*readback=Some)",
            l,
        )
        if m:
            applied_ids.add(m.group(1))
            readback_some += 1
            continue
        m = re.search(r"HELD-COMMAND intent=\[([0-9a-fx, ]+)\] state=Applied", l)
        if m:
            applied_ids.add(m.group(1))

    best = None
    for idx, pid in held:
        prior = [(c, i_, sk) for (c, i_, sk) in compiled if c < idx]
        if not prior:
            continue
        _, intents, skipped = prior[-1]
        d = done.get(pid)
        if d is None:
            continue
        applied, blocked, failed, state = d
        if (
            intents + skipped == TARGET_ASSIGNMENTS
            and applied + blocked + failed == intents
            and state == "Committed"
            and applied == intents
        ):
            best = (intents, skipped, applied, blocked, failed)
    if best:
        intents, skipped, applied, blocked, failed = best
        print(
            f"{intents} {skipped} {applied} {blocked} {failed} "
            f"{len(applied_ids)} {readback_some}"
        )
    else:
        print("NONE")


if __name__ == "__main__":
    main()
