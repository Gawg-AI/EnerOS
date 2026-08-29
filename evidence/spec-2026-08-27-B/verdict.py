#!/usr/bin/env python3
"""SPEC-B 窗口判据裁决：python3 verdict.py <window_dir> <mode>

mode ∈ {realtime, stale, creds-ok, creds-bad}
从 qemu-serial.log / broker.jsonl 提取判据，打印 PASS/FAIL 清单并给出
非零退出码（任一 FAIL）。
"""
import json
import re
import sys
from pathlib import Path

win, mode = Path(sys.argv[1]), sys.argv[2]
serial = (win / "qemu-serial.log").read_bytes().decode("utf-8", "replace")
broker_lines = []
bp = win / "broker.jsonl"
if bp.exists():
    broker_lines = [json.loads(l) for l in bp.read_text().splitlines() if l.strip()]

checks = []


def check(name, ok, detail=""):
    checks.append((name, bool(ok), detail))


# ---------- B.1 判据 ----------
root_lines = re.findall(r"\[eneros-root-task\] (TIME-EPOCH[^\r\n]*)", serial)
svc_epoch = [
    (m.group(1), m.group(2))
    for m in re.finditer(r"\[(mqtt|modbus-tcp|iec104|mms|sv|goose)\] "
                         r"TIME-EPOCH(?:-MISSING)?(?:=(\d+))?", serial)
]
missing_cnt = len(re.findall(r"TIME-EPOCH-MISSING", serial))
epoch_vals = sorted({int(v) for _, v in svc_epoch if v})

if mode == "stale":
    check("B1-C4 root 显式 MISSING（含 verdict=Stale）",
          any("verdict=Stale" in l for l in root_lines), str(root_lines[:2]))
    check("B1-C4 六协议 bin MISSING ≥6", missing_cnt >= 6, f"count={missing_cnt}")
    check("B1-C4 系统照常运行（控制链活动）",
          "DURABLE-INTENT-STORE open OK" in serial or "HEARTBEAT" in serial)
else:
    check("B1-C2 root TIME-EPOCH=<secs> 存在且>闸门下限",
          epoch_vals and min(epoch_vals) > 1_767_225_600,
          f"root_lines={root_lines[:1]} vals={epoch_vals[:2]}")
    hosts = [l["ts"] for l in broker_lines if l.get("event") == "CONNACK"]
    guests = [int(m.group(1)) for m in
              re.finditer(r"TIME-EPOCH=(\d+)", serial)]
    if hosts and guests:
        # broker 首个 CONNACK 宿主时刻 vs guest 注入的墙钟读数差 <60s。
        delta = abs(int(hosts[0]) - max(guests))
        check("B1-C2 墙钟与宿主偏差<60s", delta < 60, f"delta={delta}s")
    else:
        check("B1-C2 墙钟宿主对照可用（双方均有观测）", False,
              f"connacks={len(hosts)} guest_epochs={len(guests)}")
    check("B1 回归：无凭据模式下 broker 无 REJECT",
          not any(l.get("event") == "CONNACK-REJECT" for l in broker_lines))

# ---------- B.2 判据 ----------
accepts = [l for l in broker_lines if l.get("event") == "CONNECT-CRED"]
rejects = [l for l in broker_lines if l.get("event") == "CONNACK-REJECT"]
if mode == "creds-ok":
    check("B2-C3 CONNACK 接受且 CONNECT-CRED 记录",
          bool(accepts), f"accepts={accepts[:1]}")
    tel = [l for l in broker_lines
           if l.get("event") == "PUBLISH"
           and "telemetry" in str(l.get("topic"))]
    check("B2-C3 telemetry 照常出站", bool(tel), f"publishes={len(tel)}")
elif mode == "creds-bad":
    check("B2-C3 错误口令被拒（reason=wrong-pass rc=4）",
          any(l.get("reason") == "wrong-pass" for l in rejects),
          f"rejects={rejects[:1]}")
    check("B2-C3 无误接受（无 accept）", not accepts)
else:
    check("B2 回归：legacy 模式 CONNECT 照常 CONNACK",
          any(l.get("event") == "CONNACK" for l in broker_lines),
          f"connacks={sum(1 for l in broker_lines if l.get('event')=='CONNACK')}")

fails = [c for c in checks if not c[1]]
for name, ok, detail in checks:
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f" —— {detail}" if detail else ""))
print(f"VERDICT dir={win.name} mode={mode} pass={len(checks)-len(fails)}"
      f"/{len(checks)}")
sys.exit(1 if fails else 0)
