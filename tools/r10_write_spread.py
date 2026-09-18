#!/usr/bin/env python3
"""R10 门 A5 三资产写分布：输出 `n0x200 spread0x200 n0x201 spread0x201 n0x202 spread0x202`。"""
import json
import sys


def stat(path, addr):
    ts = []
    try:
        fh = open(path)
    except FileNotFoundError:
        return 0, 0
    for l in fh:
        try:
            e = json.loads(l)
        except Exception:
            continue
        if e.get("event") == "WRITE" and e.get("addr") == addr:
            ts.append(e["ts"])
    return len(ts), int(max(ts) - min(ts)) if ts else 0


n0, s0 = stat(sys.argv[1], "0x200")
n1, s1 = stat(sys.argv[1], "0x201")
n2, s2 = stat(sys.argv[1], "0x202")
print(n0, s0, n1, s1, n2, s2)
