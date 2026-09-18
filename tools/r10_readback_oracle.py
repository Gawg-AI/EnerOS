#!/usr/bin/env python3
"""R10 门 A4b 读回判据（2026-09-18 重写：序列敏感版）。

背景（蓝图 08 §250 / 手动复查 Spec-5）：旧判据按「地址 + 值集合」匹配，
写入之前的读取也能通过——`readback-before-write.jsonl`（全部读在写前）
旧判据输出 2 达通过线，属误放行。本 oracle 按**文件序**（transcript 追加
序 = 时间序；READ 行无 ts 字段）做写后关联：

  对每次 WRITE（按序），当且仅当其后存在一条**尚未被消费**的同地址、
  同值 READ，计 verified=1；每条 READ 至多消费一次（不得重复用于多次
  写）。

负例集（--selftest 内置，全部必须 0 或指定值）：
  - 写前读（无写后读）→ 0；
  - 旧读值（读值 ≠ 写值）→ 0；
  - 错误顺序（值匹配但读在写前）→ 0；
  - 错误设备（同值读在另一地址）→ 0；
  - 重复使用同一条读（两写一读）→ 仅 1。

用法：
  tools/r10_readback_oracle.py <transcript.jsonl>   # 输出 verified 数
  tools/r10_readback_oracle.py --selftest           # 负例集自检（异常退出 1）
"""
import json
import sys

READ_FUNC = "fc03"


def events(path):
    """按文件序产出 transcript 事件（追加序 = 时间序）。"""
    try:
        fh = open(path)
    except FileNotFoundError:
        return []
    for line in fh:
        try:
            yield json.loads(line)
        except Exception:
            continue


def verify(transcript_events):
    """序列敏感的写后读回验证：返回 verified 计数。"""
    # 预索引：每地址的 READ（文件序号, value）队列。
    reads_by_addr = {}
    writes = []  # (序号, addr, value)
    for idx, e in enumerate(transcript_events):
        if "addr" not in e:
            continue
        addr = e["addr"]
        if e.get("func") == READ_FUNC and e.get("event") == "READ":
            reads_by_addr.setdefault(addr, []).append((idx, e.get("value")))
        elif e.get("event") == "WRITE":
            writes.append((idx, addr, e.get("value")))
    verified = 0
    for w_idx, w_addr, w_value in writes:
        # 写后：序号 > w_idx 且值相等且未消费的最早 READ。
        queue = reads_by_addr.get(w_addr, [])
        for i, (r_idx, r_value) in enumerate(queue):
            if r_idx is None:
                continue  # 已消费
            if r_idx > w_idx and r_value == w_value:
                queue[i] = (None, r_value)  # 消费
                verified += 1
                break
    return verified


def _selftest():
    fixtures = [
        # (名, 事件列表, 期望 verified)
        ("positive-写后同址同值读", [
            {"event": "WRITE", "addr": "0x200", "value": 5},
            {"event": "READ", "func": READ_FUNC, "addr": "0x200", "value": 5},
            {"event": "WRITE", "addr": "0x201", "value": 3},
            {"event": "READ", "func": READ_FUNC, "addr": "0x201", "value": 3},
        ], 2),
        ("红例-读全部在写前（旧门误放行形）", [
            {"event": "READ", "func": READ_FUNC, "addr": "0x200", "value": 1},
            {"event": "READ", "func": READ_FUNC, "addr": "0x201", "value": 1},
            {"event": "WRITE", "addr": "0x200", "value": 1},
            {"event": "WRITE", "addr": "0x201", "value": 1},
        ], 0),
        ("红例-旧读值不匹配", [
            {"event": "WRITE", "addr": "0x200", "value": 5},
            {"event": "READ", "func": READ_FUNC, "addr": "0x200", "value": 3},
        ], 0),
        ("红例-错误顺序（值对但读在写前）", [
            {"event": "READ", "func": READ_FUNC, "addr": "0x200", "value": 7},
            {"event": "WRITE", "addr": "0x200", "value": 7},
        ], 0),
        ("红例-错误设备（同值读在另一地址）", [
            {"event": "WRITE", "addr": "0x200", "value": 5},
            {"event": "READ", "func": READ_FUNC, "addr": "0x201", "value": 5},
        ], 0),
        ("红例-同一读不得重复用于两写", [
            {"event": "WRITE", "addr": "0x200", "value": 5},
            {"event": "WRITE", "addr": "0x200", "value": 5},
            {"event": "READ", "func": READ_FUNC, "addr": "0x200", "value": 5},
        ], 1),
        ("交错多值（各自写后读，互不借用）", [
            {"event": "WRITE", "addr": "0x200", "value": 1},
            {"event": "WRITE", "addr": "0x201", "value": 2},
            {"event": "READ", "func": READ_FUNC, "addr": "0x200", "value": 1},
            {"event": "READ", "func": READ_FUNC, "addr": "0x201", "value": 2},
            {"event": "WRITE", "addr": "0x200", "value": 2},
            {"event": "READ", "func": READ_FUNC, "addr": "0x200", "value": 2},
        ], 3),
    ]
    failed = 0
    for name, evs, expect in fixtures:
        got = verify(iter(evs))
        mark = "ok" if got == expect else "FAIL"
        if got != expect:
            failed += 1
        print(f"[{mark}] {name}: verified={got} expect={expect}")
    if failed:
        print(f"SELFTEST FAIL: {failed} fixture(s)")
        sys.exit(1)
    print("SELFTEST PASS: 7/7")


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "--selftest":
        _selftest()
        return
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    print(verify(events(sys.argv[1])))


if __name__ == "__main__":
    main()
