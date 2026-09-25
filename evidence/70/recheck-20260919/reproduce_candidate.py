"""Run review-only candidate tests in a temporary copy; never edit workspace sources."""
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
with tempfile.TemporaryDirectory(prefix="eneros70-candidate-recheck-") as td:
    isolated = Path(td)
    for child in ROOT.iterdir():
        if child.name not in {".git", "target", "xtask"}:
            (isolated / child.name).symlink_to(child, target_is_directory=child.is_dir())
    pkg = isolated / "xtask"
    shutil.copytree(ROOT / "xtask", pkg, ignore=shutil.ignore_patterns("target"))
    manifest = (pkg / "Cargo.toml").read_text()
    for key, value in {"version": '"0.0.0"', "edition": '"2021"', "authors": "[]", "license": '"MIT"'}.items():
        manifest = re.sub(rf"(?m)^{key}\.workspace\s*=\s*true\s*$", f"{key} = {value}", manifest)
    manifest = re.sub(r'path\s*=\s*"(\.\./[^\"]+)"', lambda m: 'path = "' + str((ROOT / "xtask" / m[1]).resolve()) + '"', manifest)
    (pkg / "Cargo.toml").write_text(manifest + "\n[workspace]\n")
    shutil.copy2(ROOT / "Cargo.lock", pkg / "Cargo.lock")
    source = pkg / "src/evidence_chain.rs"
    content = source.read_text()
    last = content.rfind("}")
    source.write_text(content[:last] + (HERE / "candidate-redtests.rs").read_text() + content[last:])
    env = dict(os.environ, CARGO_TARGET_DIR="/tmp/eneros70-review-isolated/xtask/target")
    result = subprocess.run(["cargo", "test", "--offline", "--manifest-path", str(pkg / "Cargo.toml"), "recheck70_", "--", "--nocapture"], cwd=ROOT, env=env)
    raise SystemExit(result.returncode)
