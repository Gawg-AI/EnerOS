#!/usr/bin/env python3
"""蓝图 70 方向 7（E70-F）候选证据记录器。

把一次**真实门运行**转成三件套，全部落在
`evidence/70/70.15/<candidate_id>/`：

1. `<case>.run-record.json`——门运行记录（命令/退出码/原始日志摘要与行数/
   判据行快照）。原始 `.log` 体积在 MB 级、且 `.gitignore` 明确排除
   （`*.log`，与 2026-08-29 的历史剥离决定配套），故入库的是**摘要绑定**：
   谁拿到移交包里的原始日志，都能用它逐字节复核。
2. `<case>.envelope.json`——由 `cargo xtask evidence42 envelope` 生成并签名
   （信封格式保持**单源**，本脚本不自行拼装信封字段）。
3. `candidate-index.json`——required 全集索引（`eneros.candidate-evidence.v1`），
   绑定候选身份（提交 + 工作树 + manifest + profile + 产物摘要）。

用法：
  tools/run70-candidate-evidence.py record \\
      --case CORE-70-R08 --gate VOL70.7-GATE \\
      --log evidence/70/70.7/VOL70.7-gate-20260914.log \\
      --command "tools/run70-vol7-gate.sh (DL=700/DL_T2=420)" \\
      --scope L3 --profile qemu-test \\
      --artifact target/eneros/artifacts/qemu-test/eneros-qemu-test.raw \\
      --verdict-line "VOL70.7-GATE: PASS" \\
      --oracle "external qmp + serial verdict lines; 双镜像 A/B；断电前 commit" \\
      --positive "boot:PASS:BT slot=B secver seq 推进" \\
      --negative "skip-trial:FAIL:无 trial 直接确认被拒"

  tools/run70-candidate-evidence.py blocked --case CORE-70-R11 \\
      --blocker EXT-70-HW --reason "实板未到货，无 bring-up 记录"

  tools/run70-candidate-evidence.py status [--json]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
EVID_DIR = ROOT / "evidence/70/70.15"
MATRIX_ID = "core-70"


def die(msg: str) -> "None":
    print(f"run70-candidate-evidence: {msg}", file=sys.stderr)
    raise SystemExit(2)


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def rel_to_root(path: Path) -> str:
    try:
        rel = path.resolve().relative_to(ROOT.resolve())
    except ValueError:
        die(f"path outside repo: {path}")
    return rel.as_posix()


def tree_key(tree_lin: dict) -> str:
    """与 xtask `evidence_chain::candidate_tree_key` 同公式（单源在 Rust 侧，
    本处只复刻存储键；两侧都在同一提交里，改动须同时进行）。"""
    h = hashlib.sha256()
    h.update(b"eneros.candidate-tree.v1")
    h.update(b"\x00")
    h.update(tree_lin["source_commit"].encode())
    h.update(b"\x00")
    h.update(tree_lin["dirty_tree_digest"].encode())
    h.update(b"\x00")
    h.update(tree_lin["toolchain_digest"].encode())
    return h.hexdigest()


def lineage(profile: str, artifact: "Path | None") -> dict:
    """art 为 None ⇒ **树级**身份（不含产物摘要；索引用）。"""
    out = ROOT / (
        "target/eneros/evidence/70/lineage-candidate.json"
        if artifact is not None
        else "target/eneros/evidence/70/lineage-tree.json"
    )
    out.parent.mkdir(parents=True, exist_ok=True)
    cmd = ["cargo", "xtask", "evidence42", "lineage", "--profile", profile]
    if artifact is not None:
        cmd += ["--artifact", str(artifact)]
    cmd += ["--out", str(out)]
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        die(f"lineage failed: {r.stderr.strip() or r.stdout.strip()}")
    return json.loads(out.read_text())


def load_index(candidate_id: str) -> tuple[Path, dict]:
    cand_dir = EVID_DIR / candidate_id
    cand_dir.mkdir(parents=True, exist_ok=True)
    path = cand_dir / "candidate-index.json"
    if path.is_file():
        return path, json.loads(path.read_text())
    return path, {}


def save_index(path: Path, index: dict) -> None:
    path.write_text(json.dumps(index, indent=2, ensure_ascii=False) + "\n")


def verdict_lines(log: Path) -> list[str]:
    """判据行快照：门的 ✓/✗ 行 + 最终判据行（原始日志摘要之外的可读面）。

    判据段字母通用（vol 系 [Rn]、R10 专属门 [An]/[Bn]/[Cn]——2026-09-17
    随 R10-HELD 门引入）；终判行含 R10-HELD-GATE 家族。"""
    pat = re.compile(
        r"^\[[✓✗]\]\[[A-Z]+\d+\]|^=* ?VOL70\.[0-9]+[A-Z0-9 _.-]*:|^=* ?R10-HELD-GATE:"
        r"|^CHECK-CURRENT"
        r"|^CANDIDATE-CHECK|^test result:|^candidate-check red tests:"
    )
    out = []
    for line in log.read_text(errors="replace").splitlines():
        if pat.search(line):
            out.append(line.rstrip())
    return out


def cmd_record(a: argparse.Namespace) -> int:
    log = (ROOT / a.log).resolve()
    if not log.is_file():
        die(f"gate log not found: {a.log}")
    artifact = (ROOT / a.artifact).resolve()
    if not artifact.is_file():
        die(f"candidate artifact not found: {a.artifact}")

    lin = lineage(a.profile, artifact)
    tree_lin = lineage(a.profile, None)
    # 索引用**树级键**（提交 + 工作树 + 工具链；与 xtask
    # `candidate_tree_key` 同公式）：一个候选 = 一棵树 × 一组 profile 产物。
    cid = tree_key(tree_lin)

    # 产物绑定必须**可核**而非可声称：门的 `image.sha256` 记录的是它**启动
    # 前**那份镜像（就地启动会改写镜像，故运行后的文件摘要不是受测对象）。
    # 未给该文件时视为"运行不改写产物"（受测摘要 = 当前文件摘要）。
    # 缺文件即明确报错，不回退成"按信任绑定"。
    ran_sha = lin["artifact_sha256"]
    if a.ran_artifact_sha_file:
        ran = ROOT / a.ran_artifact_sha_file
        if not ran.is_file():
            die(f"--ran-artifact-sha-file not found: {a.ran_artifact_sha_file}")
        hexes = re.findall(r"\b[0-9a-f]{64}\b", ran.read_text())
        if not hexes:
            die(f"no sha256 in {a.ran_artifact_sha_file}")
        ran_sha = hexes[0]
    cand_rel = rel_to_root(EVID_DIR / cid)
    lines = verdict_lines(log)
    if a.verdict_line and not any(a.verdict_line in l for l in lines):
        die(
            f"verdict line {a.verdict_line!r} not present in {a.log} "
            "——不得记录未在原始日志中出现的判据"
        )

    raw_bytes = log.stat().st_size
    raw_sha = sha256_file(log)

    # 状态由**原始日志**决定，不由调用方声明：门自己的终态行
    # （`<gate>: PASS|FAIL` / `... WINDOW: PASS|FAIL`）是唯一来源——
    # 调用方传的 `--verdict-line` 只声明"预期哪一行存在"（传 FAIL 行即
    # 声明"本轮预期就是失败"，用于把**失败证据**也如实入库）。
    verdicts = [
        l for l in lines if re.search(r"(GATE|WINDOW|red tests): (PASS|FAIL)\b", l)
    ]
    if not verdicts:
        die(f"no gate verdict line in {a.log}——无法判定门终态（不得凭声明落状态）")
    status = (
        "PASS"
        if any(re.search(r": PASS\b", l) is not None for l in verdicts)
        and not any(re.search(r": FAIL\b", l) is not None for l in verdicts)
        else "FAIL"
    )

    record = {
        "schema": "eneros.gate-run-record.v1",
        "case_id": a.case,
        "gate_id": a.gate,
        "command": a.command,
        "exit_code": a.exit_code,
        "recorded_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "ran_artifact_sha256": ran_sha,
        "ran_artifact_sha256_source": a.ran_artifact_sha_file or "",
        "raw_log": {
            "name": log.name,
            "bytes": raw_bytes,
            "sha256": raw_sha,
            "resident": "handoff-bundle",
            "note": ".gitignore `*.log`：原始日志随移交包分发，本记录绑定其摘要",
        },
        "verdict_lines": lines,
        "verdict": status,
        "candidate": {
            "candidate_id": cid,
            "source_commit": lin["source_commit"],
            "dirty_tree_digest": lin["dirty_tree_digest"],
            "artifact_sha256": lin["artifact_sha256"],
            "profile": a.profile,
        },
    }
    rec_path = EVID_DIR / cid / f"{a.case}.run-record.json"
    rec_path.parent.mkdir(parents=True, exist_ok=True)
    rec_path.write_text(json.dumps(record, indent=2, ensure_ascii=False) + "\n")
    rec_rel = rel_to_root(rec_path)
    rec_sha = sha256_file(rec_path)

    env_path = EVID_DIR / cid / f"{a.case}.envelope.json"
    env_rel = rel_to_root(env_path)
    cmd = [
        "cargo",
        "xtask",
        "evidence42",
        "envelope",
        "--lineage",
        "target/eneros/evidence/70/lineage-candidate.json",
        "--test-id",
        a.case,
        "--scope",
        a.scope,
        "--command",
        a.command,
        "--oracle",
        a.oracle,
        "--positive",
        a.positive,
        "--negative",
        a.negative,
        "--verdict",
        status,
        "--artifact",
        f"run-record:{rec_rel}:{rec_sha}",
        "--out",
        env_rel,
    ]
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        die(f"envelope creation failed: {r.stderr.strip() or r.stdout.strip()}")
    env_sha = sha256_file(env_path)

    art_name = a.artifact_name or a.profile
    art_entry = {
        "name": art_name,
        "profile": a.profile,
        "path": rel_to_root(artifact),
        # 身份摘要 = 构建产物**当前**文件的摘要（判据可独立重算）。
        "sha256": lin["artifact_sha256"],
    }

    idx_path, index = load_index(cid)
    if not index:
        index = {
            "schema": "eneros.candidate-evidence.v1",
            "matrix_id": MATRIX_ID,
            "candidate_id": cid,
            "source_commit": tree_lin["source_commit"],
            "dirty_tree_digest": tree_lin["dirty_tree_digest"],
            "toolchain_digest": tree_lin["toolchain_digest"],
            "platform_inventory_digest": tree_lin["platform_inventory_digest"],
            "artifacts": [art_entry],
            "cases": [],
            "blocked": [],
            "exemptions": [],
        }
    else:
        # 同一候选 = 同源码 + 同工作树 + 同工具链 + 同平台事实；
        # 产物可以有多份（每 profile 一份），逐份绑定身份。
        for key in ("source_commit", "dirty_tree_digest", "toolchain_digest",
                    "platform_inventory_digest"):
            if index.get(key) != tree_lin.get(key):
                die(
                    f"index/{key} mismatch: index={index.get(key)} lineage={tree_lin.get(key)} "
                    "——换源码/换工具链是另一个候选，不得写进同一索引"
                )
        known = {x["name"]: x for x in index["artifacts"]}
        if art_name in known:
            if known[art_name]["sha256"] != lin["artifact_sha256"]:
                die(
                    f"artifact {art_name} replaced on disk: index={known[art_name]['sha256'][:12]} "
                    f"lineage={lin['artifact_sha256'][:12]}——产物换了就是新候选"
                )
            known[art_name]["path"] = art_entry["path"]
        else:
            index["artifacts"].append(art_entry)
        index["artifacts"].sort(key=lambda x: x["name"])
    index["cases"] = [c for c in index.get("cases", []) if c["case_id"] != a.case]
    index["blocked"] = [b for b in index.get("blocked", []) if b["case_id"] != a.case]
    case_entry = {
        "case_id": a.case,
        "artifact": art_name,
        "envelope": env_rel,
        "envelope_sha256": env_sha,
        "detail": f"{a.gate} // {a.scope}",
    }
    if a.ran_artifact_sha_file:
        # 就地启动会改写镜像 ⇒ 受测（启动前）字节摘要**逐 case**记录；
        # 判据要求本 case 的 run-record 自记同一个值。
        case_entry["tested_sha256"] = ran_sha
    index["cases"].append(case_entry)
    index["cases"].sort(key=lambda c: c["case_id"])
    save_index(idx_path, index)

    print(f"[candidate-evidence] case={a.case} status={status} candidate={cid[:12]}…")
    print(f"[candidate-evidence] run-record → {rec_rel}")
    print(f"[candidate-evidence] envelope   → {env_rel}")
    print(f"[candidate-evidence] index      → {rel_to_root(idx_path)}")
    return 0 if status == "PASS" else 1


def cmd_blocked(a: argparse.Namespace) -> int:
    artifact = (ROOT / a.artifact).resolve()
    _ = lineage(a.profile, artifact)
    cid = tree_key(lineage(a.profile, None))
    idx_path, index = load_index(cid)
    if not index:
        die("no candidate index yet——先 record 一个 case（索引要绑定产物身份）")
    index["blocked"] = [b for b in index.get("blocked", []) if b["case_id"] != a.case]
    index["cases"] = [c for c in index.get("cases", []) if c["case_id"] != a.case]
    index["blocked"].append(
        {"case_id": a.case, "blocker": a.blocker, "reason": a.reason}
    )
    index["blocked"].sort(key=lambda b: b["case_id"])
    save_index(idx_path, index)
    print(f"[candidate-evidence] blocked case={a.case} blocker={a.blocker}")
    return 0


def cmd_status(a: argparse.Namespace) -> int:
    if not EVID_DIR.is_dir():
        print("no candidate evidence directory")
        return 1
    for cand in sorted(p for p in EVID_DIR.iterdir() if p.is_dir()):
        idx = cand / "candidate-index.json"
        if not idx.is_file():
            print(f"{cand.name[:12]}… (no index; raw files: {len(list(cand.glob('*')))} )")
            continue
        index = json.loads(idx.read_text())
        arts = ", ".join(f"{x['name']}({x['profile']})" for x in index["artifacts"])
        print(
            f"{index['candidate_id'][:12]}… artifacts=[{arts}] "
            f"cases={len(index['cases'])} blocked={len(index['blocked'])}"
        )
        for c in index["cases"]:
            print(f"    {c['case_id']}  {c['detail']}")
        for b in index["blocked"]:
            print(f"    {b['case_id']}  BLOCKED({b['blocker']}) {b['reason']}")
    r = subprocess.run(
        ["cargo", "xtask", "evidence42", "candidate-check", "--matrix", MATRIX_ID, "--report"],
        cwd=ROOT,
        capture_output=True,
        text=True,
    )
    print(r.stdout.strip())
    if r.returncode != 0:
        print(r.stderr.strip(), file=sys.stderr)
    return r.returncode


def main() -> int:
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)

    rec = sub.add_parser("record")
    rec.add_argument("--case", required=True)
    rec.add_argument("--gate", required=True)
    rec.add_argument("--log", required=True)
    rec.add_argument("--command", required=True)
    rec.add_argument("--scope", required=True)
    rec.add_argument("--profile", default="qemu-prod")
    rec.add_argument("--artifact", required=True)
    rec.add_argument("--artifact-name", default="")
    rec.add_argument("--ran-artifact-sha-file", default="")
    rec.add_argument("--verdict-line", default="")
    rec.add_argument("--exit-code", type=int, default=0)
    rec.add_argument("--oracle", required=True)
    rec.add_argument("--positive", required=True)
    rec.add_argument("--negative", required=True)
    rec.set_defaults(func=cmd_record)

    blk = sub.add_parser("blocked")
    blk.add_argument("--case", required=True)
    blk.add_argument("--blocker", required=True)
    blk.add_argument("--reason", required=True)
    blk.add_argument("--profile", default="qemu-prod")
    blk.add_argument("--artifact", required=True)
    blk.set_defaults(func=cmd_blocked)

    st = sub.add_parser("status")
    st.set_defaults(func=cmd_status)

    a = ap.parse_args()
    return a.func(a)


if __name__ == "__main__":
    raise SystemExit(main())
