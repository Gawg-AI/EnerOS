#!/usr/bin/env python3
"""Run two expected-red candidate checks against this checkout without editing it."""
from pathlib import Path
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]

with tempfile.TemporaryDirectory(prefix="eneros70-candidate-review-") as td:
    sandbox = Path(td)
    for entry in ROOT.iterdir():
        if entry.name not in ("xtask", ".git", "target"):
            (sandbox / entry.name).symlink_to(entry, target_is_directory=entry.is_dir())
    copied = sandbox / "xtask"
    shutil.copytree(ROOT / "xtask", copied)
    manifest = copied / "Cargo.toml"
    text = manifest.read_text()
    for old, new in {
        "version.workspace = true": 'version = "0.0.0"',
        "edition.workspace = true": 'edition = "2021"',
        "authors.workspace = true": "authors = []",
        "license.workspace = true": 'license = "MIT"',
    }.items():
        text = text.replace(old, new)
    text = text.replace('path = "../crates/', f'path = "{ROOT}/crates/')
    manifest.write_text(text + "\n[workspace]\n")
    shutil.copyfile(ROOT / "Cargo.lock", copied / "Cargo.lock")
    source = copied / "src/evidence_chain.rs"
    text = source.read_text()
    end = text.rfind("\n}")
    source.write_text(text[:end] + "\n" + (HERE / "candidate_red_tests.rs").read_text() + text[end:])
    result = subprocess.run([
        "cargo", "test", "--offline", "--manifest-path", str(manifest),
        "review70_", "--", "--nocapture",
    ], cwd=ROOT)
    raise SystemExit(result.returncode)
