#!/usr/bin/env python3
"""R10-B: bind the crash/recovery verdict to one unfinished durable plan.

Do not count arbitrary PLAN-HELD / Applied / Blocked lines from completed or
unrelated plans. Logs are evidence, not a control-plane clock: a witness visible
before the crash is provisional until the flushed pre-cut log is checked again.
"""

import re
import sys
from pathlib import Path


PLAN = re.compile(r"HELD-PLAN-DONE plan=\[([^]]+)\]")
HELD = re.compile(r"PLAN-HELD plan=\[([^]]+)\]")
COMMAND = re.compile(
    r"HELD-COMMAND intent=\[([^]]+)\] state=(\w+) attempts=(\d+)"
    r".*? plan=\[([^]]+)\]"
)


def commands(text):
    out = {}
    for intent, state, attempts, plan in COMMAND.findall(text):
        out.setdefault(plan, {}).setdefault(intent, []).append((state, int(attempts)))
    return out


def witness(text):
    done = set(PLAN.findall(text))
    held = set(HELD.findall(text))
    by_plan = commands(text)
    for plan in sorted(held - done):
        states = by_plan.get(plan, {})
        applied = {
            iid: events[-1][1]
            for iid, events in states.items()
            if events[-1][0] == "Applied"
        }
        pending = {
            iid for iid, events in states.items() if events[-1][0] == "Pending"
        }
        if applied and pending:
            return plan, applied, pending
    return None


def verify(pre, post):
    candidate = witness(pre)
    if candidate is None:
        return False, "B0: no single unfinished plan with Applied prefix AND Pending suffix"
    plan, applied, pending = candidate
    after = commands(post).get(plan, {})
    for iid, attempts in applied.items():
        events = after.get(iid, [])
        if not events or any(state != "Applied" or n != attempts for state, n in events):
            return False, f"B3: prefix state/attempts changed: {iid}"
    for iid in pending:
        events = after.get(iid, [])
        if (not events or events[-1] != ("Blocked", 0)
                or any(state not in ("Pending", "Blocked") or n != 0
                       for state, n in events)):
            return False, f"B2: pending intent not cancelled without delivery: {iid}"
    return True, f"B0/B2/B3: plan={plan} applied={len(applied)} cancelled={len(pending)}"


def _line(plan, intent, state, attempts=0):
    return (f"[svc-control-dispatch] HELD-COMMAND intent=[{intent}] "
            f"state={state} attempts={attempts} readback=None "
            f"reason=- plan=[{plan}] dev=0x1 pt=10 val=0\n")


def selftest():
    held = "[svc-control-dispatch] PLAN-HELD plan=[aa] batches=2 intents=2\n"
    prefix = _line("aa", "01", "Applied", 1)
    suffix = _line("aa", "02", "Pending", 0)
    post = _line("aa", "01", "Applied", 1) + _line("aa", "02", "Blocked", 0)
    done = "[svc-control-dispatch] HELD-PLAN-DONE plan=[aa] state=Committed\n"
    cases = [
        ("complete plan cannot witness B0", held + prefix + suffix + done, post, False),
        ("different plans cannot combine evidence", held + prefix +
         "[svc-control-dispatch] PLAN-HELD plan=[bb] batches=2 intents=2\n" +
         _line("bb", "02", "Pending"), post, False),
        ("pending-only is not Applied prefix", held + suffix, post, False),
        ("same unfinished plan survives recovery", held + prefix + suffix, post, True),
        ("unrelated Blocked cannot cancel suffix", held + prefix + suffix,
         _line("aa", "01", "Applied", 1) + _line("bb", "02", "Blocked"), False),
        ("attempts changed is re-delivery", held + prefix + suffix,
         _line("aa", "01", "Applied", 2) + _line("aa", "02", "Blocked"), False),
        ("Executing state disallowed even if later Applied", held + prefix + suffix,
         _line("aa", "01", "Executing", 2) + post, False),
        ("Applied prefix cannot transition later to new attempts", held + prefix + suffix,
         post + _line("aa", "01", "Applied", 2), False),
        ("Pending suffix cannot execute before Blocked", held + prefix + suffix,
         _line("aa", "02", "Executing", 1) + post, False),
        ("Pending suffix cannot apply after Blocked", held + prefix + suffix,
         post + _line("aa", "02", "Applied", 1), False),
        ("Pending before cancelled is allowed without delivery", held + prefix + suffix,
         _line("aa", "02", "Pending", 0) + post, True),
    ]
    for label, pre, after, expected in cases:
        observed, detail = verify(pre, after)
        assert observed == expected, f"{label}: {detail}"
        print(f"[selftest] PASS {label}")
    print("[selftest] all PASS")


def main():
    if len(sys.argv) == 2 and sys.argv[1] == "--selftest":
        selftest()
        return 0
    if len(sys.argv) not in (3, 4) or sys.argv[1] not in ("witness", "verify"):
        print("usage: r10_b_witness.py --selftest | witness PRE | verify PRE POST", file=sys.stderr)
        return 2
    try:
        pre = Path(sys.argv[2]).read_text(errors="replace")
        if sys.argv[1] == "witness" and len(sys.argv) == 3:
            found = witness(pre)
            if found is None:
                print("B0: no unfinished Applied+Pending witness", file=sys.stderr)
                return 1
            print(f"B0: plan={found[0]} applied={len(found[1])} pending={len(found[2])}")
            return 0
        if sys.argv[1] == "verify" and len(sys.argv) == 4:
            ok, detail = verify(pre, Path(sys.argv[3]).read_text(errors="replace"))
            print(detail, file=sys.stderr if not ok else sys.stdout)
            return 0 if ok else 1
    except OSError as exc:
        print(f"R10-B evidence unavailable: {exc}", file=sys.stderr)
        return 2
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
