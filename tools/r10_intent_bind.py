#!/usr/bin/env python3
"""R10 门外部读回/写分布的**逐计划归属**判据（recheck RR2 / F70-05 §8.2.5-7）。

背景（复查缺陷 RR2）：A4b/A5 旧谓词对**整份** modbus transcript 计数再与
选中计划的 UNIQ 比较——transcript 无 plan/intent ID 时，另一计划 Q 的
写入/读回可以证明计划 P（Q-only 夹具实测误通过）。协议事务与 intent 的
绑定从此走**真实投递 trace**：

  1. 串口侧：HELD-COMMAND 行携带完整 intent id + plan= 归属 + 绑定锚
     `dev=0x.. pt=.. val=..`（dispatch 观测行，F70-05/recheck RR2）；
  2. 点表侧：configs/points.toml 把 (device_id, point_id) 映射到
     reg_addr/scale/offset（映射单源——与写执行路径 write_point 同表）；
  3. transcript 侧：每次 WRITE 只绑定到**期望该 (addr, 值) 的本计划
     intent**（FIFO）；写后的同址同值 READ 每条至多消费一次——一次
     READ 只能证明**一个** intent；写前读/错值/错误地址不证明任何 intent。

通过线：选中计划（与 r10_plan_accounting.select 同谓词）的全部 Applied
intent 均获得「自己期望的写 + 写后读回」的外部证明。

输出：
  行1 `proven total intents`——proven = 有外部写+读回证明的 intent 数；
  行2 `n0x200 spread0x200 n0x201 spread0x201 n0x202 spread0x202`——
  选中计划**绑定写**的逐资产次数与时间跨度（秒，取整）。
退出码：0 = 全部证明；1 = 有未证明 intent（含 Q-only 借数形：proven=0）；
2 = 输入不可判（Applied 行缺绑定锚的历史日志 / 点表缺映射 / 矛盾）。

用法：
  tools/r10_intent_bind.py <serial-log> <transcript.jsonl> [--points <toml>]
  tools/r10_intent_bind.py --selftest
"""
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import r10_plan_accounting as acct  # noqa: E402  选择谓词单源复用

R10_ASSET_ADDRS = ("0x200", "0x201", "0x202")
READ_FUNC = "fc03"
DEFAULT_POINTS = Path(__file__).resolve().parent.parent / "configs/points.toml"


class Reject(Exception):
    """输入不可判（exit 2）——不猜、不补造归属。"""


def parse_id(text):
    return acct.parse_id(text)


def pid_hex(pid):
    return acct.pid_hex(pid)


def load_points(path):
    """(device_id, point_id) → (reg_addr_hex, scale, offset)。"""
    import tomllib

    with open(path, "rb") as fh:
        data = tomllib.load(fh)
    table = {}
    for point in data.get("point", []):
        modbus = point.get("addr", {}).get("modbus")
        if not modbus:
            continue
        key = (int(point["device_id"]), int(point["point_id"]))
        table[key] = (
            hex(int(modbus["reg_addr"])),
            float(point.get("scale", 1.0)),
            float(point.get("offset", 0.0)),
        )
    return table


def collect_intents(lines, pid):
    """选中计划 P 的 Applied intent 绑定锚：intent id → (addr, raw)。

    raw = round((val - offset) / scale)——与写执行路径
    `mapping.from_engineering` 同一换算（scale/offset 来自点表）。"""
    table = None  # lazy
    out = {}
    order = []
    pat = re.compile(
        r"HELD-COMMAND intent=\[([0-9a-fx, ]+)\] state=(\w+)(.*)"
    )
    for lineno, l in enumerate(lines, 1):
        m = pat.search(l)
        if not m or m.group(2) != "Applied":
            continue
        pm = re.search(r"plan=\[([0-9a-fx, ]+)\]", m.group(3))
        line_pid = parse_id(pm.group(1)) if pm else None
        if line_pid != pid:
            continue
        iid = parse_id(m.group(1))
        if iid is None:
            raise Reject(f"line {lineno}: Applied intent 无完整 16B id")
        dm = re.search(r"dev=0x([0-9a-fA-F]+) pt=(\d+) val=([0-9.eE+-]+)", m.group(3))
        if not dm:
            # 历史形态（无绑定锚）不能建立协议事务归属——不补造。
            raise Reject(
                f"line {lineno}: Applied 行缺 dev/pt/val 绑定锚（历史日志不能用于外部归属判据）"
            )
        if table is None:
            table = load_points(POINTS_PATH)
        key = (int(dm.group(1), 16), int(dm.group(2)))
        if key not in table:
            raise Reject(f"line {lineno}: (dev,pt)={key} 不在点表——无法映射寄存器")
        addr, scale, offset = table[key]
        if scale == 0.0:
            raise Reject(f"line {lineno}: 点 {key} scale=0 非法")
        raw = round((float(dm.group(3)) - offset) / scale)
        anchor = (addr, int(raw))
        if iid in out and out[iid] != anchor:
            raise Reject(f"line {lineno}: intent {iid[:4]} 绑定锚矛盾")
        if iid not in out:
            out[iid] = anchor
            order.append(iid)
    return out, order


def bind(intents, order, transcript_events):
    """把 transcript 事务绑定到 intent：返回 (proven_ids, 逐 addr 绑定写 ts)。

    序列敏感：WRITE 按序消费「期望该 (addr, raw) 的最早未绑定 intent」；
    READ 只证明「写序早于它且尚未证明」的绑定，且每条 READ 至多一次。"""
    pending = {}
    for iid in order:
        pending.setdefault(intents[iid], []).append(iid)
    awaiting = []  # [iid, w_idx]（已绑定写、等待写后读）
    proven = set()
    writes_ts = {}  # addr -> [ts]（本计划绑定写）
    for idx, e in enumerate(transcript_events):
        if e.get("event") == "WRITE":
            key = (e.get("addr"), int(e.get("value")))
            queue = pending.get(key)
            if queue:
                iid = queue.pop(0)
                awaiting.append((iid, idx))
                writes_ts.setdefault(key[0], []).append(e.get("ts"))
        elif e.get("event") == "READ" and e.get("func") == READ_FUNC:
            key = (e.get("addr"), int(e.get("value")))
            for i, (iid, w_idx) in enumerate(awaiting):
                if intents[iid] == key and w_idx < idx:
                    proven.add(iid)
                    awaiting.pop(i)
                    break
    return proven, writes_ts


def spread_line(writes_ts):
    parts = []
    for addr in R10_ASSET_ADDRS:
        ts = writes_ts.get(addr, [])
        parts.append(str(len(ts)))
        parts.append(str(int(max(ts) - min(ts))) if len(ts) >= 2 else "0")
    return " ".join(parts)


POINTS_PATH = DEFAULT_POINTS


def run(serial_lines, transcript_events, points_path):
    global POINTS_PATH
    POINTS_PATH = points_path
    picked = acct.select(serial_lines)
    if picked is None:
        raise Reject("无满足守恒的完整计划（select 同谓据——先过守恒再谈外部归属）")
    pid, row = picked
    intents, order = collect_intents(serial_lines, pid)
    total = len(intents)
    proven, writes_ts = bind(intents, order, transcript_events)
    return pid, row, total, proven, writes_ts


def _selftest():
    import os
    import tempfile

    failures = []

    def check(label, cond, detail=""):
        print(f"[selftest] {'PASS' if cond else 'FAIL'} {label}{'' if cond else ': ' + detail}")
        if not cond:
            failures.append(label)

    def _hid(tag, n=16):
        return ", ".join(f"{(tag + i) & 0xFF:02x}" for i in range(n))

    A = _hid(0xA1)

    def cmd(iid, dev, pt, val):
        return (
            f"[svc-control-dispatch] HELD-COMMAND intent=[{iid}] state=Applied "
            f"attempts=1 readback=Some(1.0) reason=- plan=[{A}] dev=0x{dev:x} pt={pt} val={val}"
        )

    def compiled(i, s):
        return (
            f"[svc-control-dispatch] PLAN-COMPILED batches=3 intents={i} skipped={s} manual=0 "
            f"compiler=[11, 22, 33, 44] batch=[55, 66, 77, 88] plan=[{A}]"
        )

    serial = [
        compiled(6, 6),
        f"[svc-control-dispatch] PLAN-HELD plan=[{A}] batches=3 intents=6",
    ]
    # 6 intents：dev1/pt10 → 0x200 ×3；dev8/pt1009 → 0x201 ×3（临时点表）。
    ids = [_hid(0x10 + k) for k in range(6)]
    for k, iid in enumerate(ids):
        dev, pt = (1, 10) if k < 3 else (8, 1009)
        serial.append(cmd(iid, dev, pt, 1.0))
    serial.append(
        f"[svc-control-dispatch] HELD-PLAN-DONE plan=[{A}] batches=3 "
        f"applied=6 blocked=0 failed=0 state=Committed"
    )

    points = """
[[point]]
point_id = 10
device_id = 1
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 2, reg_addr = 0x0200, func_code = 3 }
data_type = "control"
scale = 1.0
offset = 0.0

[[point]]
point_id = 1009
device_id = 8
protocol = "modbus-tcp"
addr.modbus = { slave_addr = 2, reg_addr = 0x0201, func_code = 3 }
data_type = "control"
scale = 1.0
offset = 0.0
"""

    def W(addr, val, ts):
        return {"event": "WRITE", "func": "fc06", "addr": addr, "ts": ts, "value": val}

    def R(addr, val):
        return {"event": "READ", "func": READ_FUNC, "addr": addr, "value": val}

    with tempfile.TemporaryDirectory(prefix="r10-bind-") as td:
        pts = Path(td) / "points.toml"
        pts.write_text(points)

        # 正例：6 intents 各自的写后读（值 1）→ proven 6/6。
        good = []
        for k in range(6):
            addr = "0x200" if k < 3 else "0x201"
            good.append(W(addr, 1, 1000.0 + 7 * k))
            good.append(R(addr, 1))
        pid, row, total, proven, wt = run(serial, good, pts)
        check("正例：全部 intent 获外部写+读回证明",
              total == 6 and len(proven) == 6 and row["intents"] == 6,
              f"total={total} proven={len(proven)}")
        check("正例：绑定写按资产分布", spread_line(wt).startswith("3 "), spread_line(wt))

        # RR2 核心对抗例（recheck Q-only 夹具同构）：transcript 全部是另一
        # 计划 Q 的值 99 写入/读回——P 的 intent 一个都不得被证明。
        qonly = []
        for k in range(12):
            addr = "0x200" if k % 3 == 0 else ("0x201" if k % 3 == 1 else "0x202")
            qonly.append(W(addr, 99, 1000.0 + 5 * k))
            qonly.append(R(addr, 99))
        pid, row, total, proven, wt = run(serial, qonly, pts)
        check("RR2：Q-only 事务不得证明 P（proven=0）",
              len(proven) == 0 and total == 6,
              f"proven={len(proven)}")
        check("RR2：Q-only 绑定写计数为 0", spread_line(wt) == "0 0 0 0 0 0", spread_line(wt))

        # 混入干扰：Q 的值 99 事务夹在 P 的正确事务之间——互不借用。
        mixed = [W("0x200", 99, 1.0), R("0x200", 99)]
        for k in range(6):
            addr = "0x200" if k < 3 else "0x201"
            mixed.append(W(addr, 1, 10.0 + k))
            mixed.append(R(addr, 1))
        pid, row, total, proven, wt = run(serial, mixed, pts)
        check("混入他计划事务不借用", len(proven) == 6, f"proven={len(proven)}")

        # 写前读：读在写前不证明。
        before = [R("0x200", 1), R("0x201", 1)] + [
            x for k in range(6) for x in (W("0x200" if k < 3 else "0x201", 1, 5.0 + k),)
        ]
        pid, row, total, proven, wt = run(serial, before, pts)
        check("写前读不证明", len(proven) == 0, f"proven={len(proven)}")

        # 读复用：一读不得证明两写。
        reuse = [W("0x200", 1, 1.0), W("0x200", 1, 2.0), R("0x200", 1)]
        pid, row, total, proven, wt = run(serial, reuse, pts)
        check("同一读至多证明一个 intent", len(proven) == 1, f"proven={len(proven)}")

        # 历史形态（无 dev/pt/val 绑定锚）→ Reject。
        legacy = [
            compiled(6, 6),
            f"[svc-control-dispatch] PLAN-HELD plan=[{A}] batches=3 intents=6",
            f"[svc-control-dispatch] HELD-COMMAND intent=[{ids[0]}] state=Applied "
            f"attempts=1 readback=Some(1.0) reason=- plan=[{A}]",
            f"[svc-control-dispatch] HELD-PLAN-DONE plan=[{A}] batches=3 "
            f"applied=6 blocked=0 failed=0 state=Committed",
        ]
        try:
            run(legacy, good, pts)
            check("历史形态（无绑定锚）拒绝 exit 2", False, "未拒绝")
        except Reject:
            check("历史形态（无绑定锚）拒绝 exit 2", True)

    if failures:
        print(f"[selftest] {len(failures)} FAILED")
        return 1
    print("[selftest] all PASS")
    return 0


def main():
    global POINTS_PATH
    if len(sys.argv) == 2 and sys.argv[1] == "--selftest":
        raise SystemExit(_selftest())
    args = [a for a in sys.argv[1:] if not a.startswith("--points")]
    points_arg = None
    if "--points" in sys.argv:
        points_arg = Path(sys.argv[sys.argv.index("--points") + 1])
    if len(args) != 2:
        print(__doc__)
        raise SystemExit(2)
    POINTS_PATH = points_arg or DEFAULT_POINTS
    try:
        lines = open(args[0]).read().splitlines()
        events = []
        with open(args[1]) as fh:
            for line in fh:
                try:
                    events.append(json.loads(line))
                except Exception:
                    continue
        pid, row, total, proven, writes_ts = run(lines, events, POINTS_PATH)
    except Reject as e:
        print(f"r10_intent_bind: REJECT {e}", file=sys.stderr)
        raise SystemExit(2)
    print(f"{len(proven)} {total} {row['intents']}")
    print(spread_line(writes_ts))
    if total >= 1 and len(proven) == total == row["intents"]:
        raise SystemExit(0)
    unproven = total - len(proven)
    print(
        f"r10_intent_bind: plan {pid_hex(pid)} 外部证明不足 proven={len(proven)} "
        f"total={total} intents={row['intents']}（缺 {unproven}）——跨计划事务不得补数",
        file=sys.stderr,
    )
    raise SystemExit(1)


if __name__ == "__main__":
    main()
