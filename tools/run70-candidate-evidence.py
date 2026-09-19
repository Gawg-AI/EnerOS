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
    """完整写入后原子切换（tmp + rename）：写到一半不会留下半截索引。"""
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_text(json.dumps(index, indent=2, ensure_ascii=False) + "\n")
    tmp.replace(path)


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


# recheck RR3 重订：显式别名 + 完整终态 token 匹配（不做任意前后缀推断）。
GATE_LINE_ALIASES: dict[str, tuple[str, ...]] = {
    # 复合 gate 在判据行里的行首名（`candidate-check red tests: PASS`）。
    "evidence42-candidate-check": ("candidate-check",),
    # 子窗 gate 的判据行携带描述段（vol2 门 R03 断电窗终态行）。
    "VOL70.2-R03": ("VOL70.2-R03-POWER-CUT",),
}

_TERMINAL_MARKERS = ("-GATE:", " GATE:", " QEMU WINDOW:", " red tests:")


def _line_terminal_parses(line: str) -> list[tuple[str, str]]:
    """把一行解析为 [(行首 gate 名, PASS|FAIL)]；解析不出 = []。

    合法形态（两端 '='/空白先剥去）：`<name>-GATE:`/`<name> GATE:`/
    `<name> QEMU WINDOW:`/`<name> red tests:`/裸 `<name>:`。name 必须是
    单 token（不含空格）；状态 token 必须**完整等于** PASS/FAIL——扩展或
    截断 token（`PASSED_NOT`）不是终态行，组合/带尾注的行整体不可判。
    """
    norm = line.strip("= ")
    out: list[tuple[str, str]] = []
    for marker in _TERMINAL_MARKERS:
        pos = norm.find(marker)
        while pos != -1:
            name = norm[:pos]
            status = norm[pos + len(marker):].strip("= ")
            if name and " " not in name and status in ("PASS", "FAIL"):
                out.append((name, status))
            pos = norm.find(marker, pos + 1)
    if ":" in norm:
        name, _, rest = norm.rpartition(":")
        status = rest.strip("= ")
        if name and " " not in name and status in ("PASS", "FAIL"):
            out.append((name, status))
    return out


def gate_terminal_status(line: str, gate: str):
    """一行是否是指定 gate 的终态行：True=PASS / False=FAIL / None=无关。

    F70-01（R4/E05）+ recheck RR3：case 的成功只能由**指定 gate 自己的
    终态行**证明。归属 = 行首名与 gate（或其显式别名）**整名相等**；
    终态 token 完整匹配。旧实现的 PASS 子串匹配（`GATE: PASSED_NOT`
    命中）与 gate 前缀/任意 '-' 后缀段推断（`VOL70.2-R03-…GATE: PASS`
    证明 `VOL70.2`、`HELD-GATE: PASS` 证明 `R10-HELD`）已退役。与 xtask
    `evidence_chain::gate_terminal_status` 单源同构，两侧必须同时改。"""
    names = {gate, *GATE_LINE_ALIASES.get(gate, ())}
    states = [s for n, s in _line_terminal_parses(line) if n in names]
    if not states:
        return None
    if "FAIL" in states:
        return False
    return True


def gate_status_from_log(lines: list[str], gate: str) -> "tuple[str, list[str]] | tuple[None, list[str]]":
    """从判据行快照判定**指定 gate** 的终态。

    返回 (status, gate_lines)：无该 gate 终态行时 status=None（缺终态，
    不得凭声明落状态）；有 PASS 无 FAIL → PASS；否则 FAIL。别的 gate 的
    PASS、正文 PASS、`--verdict-line` 提示值都不参与判定。"""
    gate_lines = [l for l in lines if gate_terminal_status(l, gate) is not None]
    states = [gate_terminal_status(l, gate) for l in gate_lines]
    if not states:
        return None, gate_lines
    if any(s is True for s in states) and not any(s is False for s in states):
        return "PASS", gate_lines
    return "FAIL", gate_lines


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

    # 状态由**指定 gate 自己的终态行**决定（F70-01/R4）：声明 `--gate
    # VOL70.1` 而日志里只有 VOL70.2 的 PASS → 拒绝（缺终态）；调用方传的
    # `--verdict-line` 只是"预期哪一行存在"的提示，不参与判定。
    status, gate_lines = gate_status_from_log(lines, a.gate)
    if status is None:
        die(
            f"no terminal verdict line for gate {a.gate!r} in {a.log}"
            "——无法判定门终态（别的 gate 的 PASS 不能充当本 gate 证据）"
        )
    # PASS 必须同时有零退出码：非零退出 + PASS 行是矛盾输入，不产生
    # 成功记录（外层声明不得覆盖实际聚合退出码）。
    if status == "PASS" and a.exit_code != 0:
        die(
            f"gate {a.gate} claims PASS but --exit-code={a.exit_code}: "
            "PASS 记录要求实际聚合退出码为 0"
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
    # recheck RR4：每次运行使用**独立不可覆盖**的 run 路径（内容摘要做
    # run id——同内容重写幂等，异内容必然分文件）。旧实现把 run-record/
    # envelope 写死为 `<case>.*.json` 且在失败分支**之前**落盘：同 case
    # 先 PASS 后 FAIL 时，失败文件会顶掉索引仍引用的成功文件（摘要失配、
    # 原始运行证据丢失）。索引只在 PASS 且三件套完整后才原子切换。
    record_body = json.dumps(record, indent=2, ensure_ascii=False) + "\n"
    run_id = hashlib.sha256(record_body.encode()).hexdigest()[:16]
    rec_path = EVID_DIR / cid / f"{a.case}.{run_id}.run-record.json"
    rec_path.parent.mkdir(parents=True, exist_ok=True)
    rec_path.write_text(record_body)
    rec_rel = rel_to_root(rec_path)
    rec_sha = sha256_file(rec_path)

    env_path = EVID_DIR / cid / f"{a.case}.{run_id}.envelope.json"
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

    if status != "PASS":
        # 失败运行留档为**失败证据**（run-record + envelope 如实记 FAIL），
        # 但不得写进索引充当成功条目，也不得顶掉该 case 已有的成功条目
        # （F70-01 §4.2.8：错误或不完整运行不得替换为一条成功索引记录）。
        print(f"[candidate-evidence] case={a.case} status=FAIL candidate={cid[:12]}…")
        print(f"[candidate-evidence] run-record → {rec_rel}")
        print(f"[candidate-evidence] envelope   → {env_rel}")
        print("[candidate-evidence] index      → 未变更（失败证据不注册成功条目）")
        return 1

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
    return 0


def _selftest() -> int:
    """F70-01 记录器长期回归：修前红/修后绿的对抗用例（E04/E05/E06/E08）。

    隔离 tmp 根 + mock lineage/envelope 子进程——不落真实 evidence 目录、
    不需要 cargo。"""
    import tempfile
    from unittest.mock import patch

    failures: list[str] = []

    def check(label: str, cond: bool, detail: str = "") -> None:
        print(f"[selftest] {'PASS' if cond else 'FAIL'} {label}{'' if cond else ': ' + detail}")
        if not cond:
            failures.append(label)

    # --- 归属规则（与 xtask gate_terminal_status 同构）---
    check(
        "attrib: vol window",
        gate_terminal_status("=== VOL70.1 QEMU WINDOW: PASS ===", "VOL70.1") is True,
    )
    check("attrib: gate suffix", gate_terminal_status("VOL70.7-GATE: PASS", "VOL70.7") is True)
    check(
        "attrib: subgate",
        gate_terminal_status("VOL70.2-R03-POWER-CUT GATE: PASS", "VOL70.2-R03") is True,
    )
    check(
        "attrib: compound gate suffix",
        gate_terminal_status(
            "candidate-check red tests: PASS", "evidence42-candidate-check"
        )
        is True,
    )
    check(
        "attrib: other gate not mine (E05)",
        gate_terminal_status("=== VOL70.2 QEMU WINDOW: PASS ===", "VOL70.1") is None,
    )
    check(
        "attrib: near-prefix not mine",
        gate_terminal_status("VOL70.10-GATE: PASS", "VOL70.1") is None,
    )
    check("attrib: fail terminal", gate_terminal_status("VOL70.7-GATE: FAIL", "VOL70.7") is False)
    check("attrib: plain line not terminal", gate_terminal_status("VOL70.1 boot ok", "VOL70.1") is None)
    # recheck RR3 对抗例：错误子门/扩展 token/后缀段推断必须全部拒绝。
    check(
        "attrib RR3: subgate line does not prove parent",
        gate_terminal_status("VOL70.2-R03-POWER-CUT GATE: PASS", "VOL70.2") is None,
    )
    check(
        "attrib RR3: extended PASS token rejected",
        gate_terminal_status("VOL70.1-GATE: PASSED_NOT", "VOL70.1") is None,
    )
    check(
        "attrib RR3: suffix segment does not prove compound gate",
        gate_terminal_status("HELD-GATE: PASS", "R10-HELD") is None,
    )
    check(
        "attrib RR3: trailing text after token rejected",
        gate_terminal_status("VOL70.7-GATE: PASS extra", "VOL70.7") is None,
    )
    check(
        "attrib RR3: bare name terminal kept (compound gate fixtures)",
        gate_terminal_status("TEST-GATE: FAIL", "TEST-GATE") is False,
    )

    st, gl = gate_status_from_log(["=== VOL70.2 QEMU WINDOW: PASS ==="], "VOL70.1")
    check("gate_status: wrong gate absent → None", st is None and gl == [])
    st, _ = gate_status_from_log(["VOL70.1-GATE: PASS", "VOL70.2-GATE: FAIL"], "VOL70.1")
    check("gate_status: other gate FAIL ignored", st == "PASS")
    st, _ = gate_status_from_log(["VOL70.1-GATE: PASS", "VOL70.1-GATE: FAIL"], "VOL70.1")
    check("gate_status: conflict → FAIL", st == "FAIL")

    # --- 记录端行为（隔离根 + mock；真实 verdict/exit 判定逻辑）---
    with tempfile.TemporaryDirectory(prefix="run70-selftest-") as td:
        old_root, old_evid = ROOT, EVID_DIR
        globals()["ROOT"], globals()["EVID_DIR"] = Path(td), Path(td) / "evidence/70/70.15"

        def lin(profile, artifact):
            return dict(
                source_commit="a" * 40,
                dirty_tree_digest="b" * 64,
                toolchain_digest="c" * 64,
                platform_inventory_digest="d" * 64,
                artifact_sha256=sha256_file(artifact) if artifact else "",
            )

        def emit(cmd, **kw):
            # 真实子进程以 cwd=ROOT 运行、--out 是仓库相对路径；mock 同样
            # 解析到当前（已隔离的）ROOT 下。
            out = globals()["ROOT"] / cmd[cmd.index("--out") + 1]
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_text(json.dumps({"verdict": cmd[cmd.index("--verdict") + 1]}))
            return argparse.Namespace(returncode=0, stdout="", stderr="")

        # E04：指定 gate PASS 行 + 实际 exit=1 → 必须拒绝（不产生成功记录）。
        with patch.object(sys.modules[__name__], "lineage", lin), patch.object(
            sys.modules[__name__].subprocess, "run", emit
        ):
            (globals()["ROOT"] / "run.log").write_text("VOL70.1-GATE: PASS\n")
            (globals()["ROOT"] / "artifact.raw").write_bytes(b"test artifact")
            a = argparse.Namespace(
                log="run.log", artifact="artifact.raw", profile="qemu-test",
                ran_artifact_sha_file="", verdict_line="", case="CORE-70-R01",
                gate="VOL70.1", command="real gate command", exit_code=1,
                scope="L2", oracle="external test", positive="ok:PASS:ok",
                negative="bad:FAIL:rejected", artifact_name="test",
            )
            try:
                cmd_record(a)
                check("recorder E04: nonzero exit rejected", False, "cmd_record returned")
            except SystemExit as e:
                check("recorder E04: nonzero exit rejected", e.code == 2)

        # E05：声明 VOL70.1、日志只有 VOL70.2 的 PASS → 拒绝。
        with patch.object(sys.modules[__name__], "lineage", lin), patch.object(
            sys.modules[__name__].subprocess, "run", emit
        ):
            (globals()["ROOT"] / "run.log").write_text("VOL70.2-GATE: PASS\n")
            a = argparse.Namespace(
                log="run.log", artifact="artifact.raw", profile="qemu-test",
                ran_artifact_sha_file="", verdict_line="", case="CORE-70-R01",
                gate="VOL70.1", command="real gate command", exit_code=0,
                scope="L2", oracle="external test", positive="ok:PASS:ok",
                negative="bad:FAIL:rejected", artifact_name="test",
            )
            try:
                cmd_record(a)
                check("recorder E05: wrong gate rejected", False, "cmd_record returned")
            except SystemExit as e:
                check("recorder E05: wrong gate rejected", e.code == 2)

        # E06（记录端）：缺任何 gate 终态行 → 拒绝。
        with patch.object(sys.modules[__name__], "lineage", lin), patch.object(
            sys.modules[__name__].subprocess, "run", emit
        ):
            (globals()["ROOT"] / "run.log").write_text("VOL70.1 boot ok\n[✓][R1] fine\n")
            a = argparse.Namespace(
                log="run.log", artifact="artifact.raw", profile="qemu-test",
                ran_artifact_sha_file="", verdict_line="", case="CORE-70-R01",
                gate="VOL70.1", command="real gate command", exit_code=0,
                scope="L2", oracle="external test", positive="ok:PASS:ok",
                negative="bad:FAIL:rejected", artifact_name="test",
            )
            try:
                cmd_record(a)
                check("recorder E06: no terminal rejected", False, "cmd_record returned")
            except SystemExit as e:
                check("recorder E06: no terminal rejected", e.code == 2)

        # E08：真实负例门（被测进程预期退出非零，门驱动断言全过、门自身
        # exit=0 + 终态 PASS）→ 正常接纳；日志含 "FAIL" 字样不误拒。
        with patch.object(sys.modules[__name__], "lineage", lin), patch.object(
            sys.modules[__name__].subprocess, "run", emit
        ):
            (globals()["ROOT"] / "run.log").write_text(
                "[✗][N1] 预期拒绝样例 skip-trial:FAIL:被拒（负例按预期失败）\n"
                "VOL70.1-GATE: PASS\n"
            )
            a = argparse.Namespace(
                log="run.log", artifact="artifact.raw", profile="qemu-test",
                ran_artifact_sha_file="", verdict_line="", case="CORE-70-R01",
                gate="VOL70.1", command="real gate command", exit_code=0,
                scope="L2", oracle="external test", positive="ok:PASS:ok",
                negative="bad:FAIL:rejected", artifact_name="test",
            )
            rc = cmd_record(a)
            check("recorder E08: negative-run gate accepted", rc == 0, f"rc={rc}")
            recs = list((globals()["EVID_DIR"]).glob("*/CORE-70-R01.*.run-record.json"))
            check("recorder E08: record written", len(recs) == 1)
            if recs:
                verdict = json.loads(recs[0].read_text())["verdict"]
                check("recorder E08: verdict PASS recorded", verdict == "PASS", verdict)
            idxs = list((globals()["EVID_DIR"]).glob("*/candidate-index.json"))
            check("recorder E08: index registered", len(idxs) == 1)
            # 成功三件套快照（RR4：失败重跑不得破坏）。
            idx_before = json.loads(idxs[0].read_text())
            env_entry = next(c for c in idx_before["cases"] if c["case_id"] == "CORE-70-R01")
            env_file = globals()["ROOT"] / env_entry["envelope"]
            env_bytes_before = env_file.read_bytes()
            rec_files_before = sorted(
                p.name for p in idxs[0].parent.glob("CORE-70-R01.*.run-record.json")
            )

            # 失败门（终态 FAIL）→ 留失败证据、返回 1、不写成功索引。
            (globals()["ROOT"] / "run.log").write_text("VOL70.1-GATE: FAIL\n")
            a2 = argparse.Namespace(
                log="run.log", artifact="artifact.raw", profile="qemu-test",
                ran_artifact_sha_file="", verdict_line="", case="CORE-70-R01",
                gate="VOL70.1", command="real gate command", exit_code=1,
                scope="L2", oracle="external test", positive="ok:PASS:ok",
                negative="bad:FAIL:rejected", artifact_name="test",
            )
            rc2 = cmd_record(a2)
            check("recorder FAIL: returns 1", rc2 == 1, f"rc={rc2}")
            idx2 = json.loads(idxs[0].read_text())
            check(
                "recorder FAIL: index untouched by failure",
                any(c["case_id"] == "CORE-70-R01" for c in idx2["cases"]),
                "success entry vanished",
            )
            # RR4：旧成功信封逐字节保持，索引摘要仍与盘上文件吻合。
            check(
                "recorder RR4: success envelope bytes preserved",
                env_file.read_bytes() == env_bytes_before,
                "failure run overwrote the indexed success envelope",
            )
            check(
                "recorder RR4: index envelope digest still matches disk",
                sha256_file(env_file) == env_entry["envelope_sha256"],
                "indexed digest diverged after failed rerun",
            )
            rec_files_after = sorted(
                p.name for p in idxs[0].parent.glob("CORE-70-R01.*.run-record.json")
            )
            check(
                "recorder RR4: failure run archived separately",
                len(rec_files_after) == len(rec_files_before) + 1
                and set(rec_files_before) <= set(rec_files_after),
                f"before={rec_files_before} after={rec_files_after}",
            )

        globals()["ROOT"], globals()["EVID_DIR"] = old_root, old_evid

    if failures:
        print(f"[selftest] {len(failures)} FAILED")
        return 1
    print("[selftest] all PASS")
    return 0


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

    self = sub.add_parser("selftest", help="F70-01 记录器对抗回归（离线）")
    self.set_defaults(func=lambda _a: _selftest())

    a = ap.parse_args()
    return a.func(a)


if __name__ == "__main__":
    raise SystemExit(main())
