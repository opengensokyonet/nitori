#!/usr/bin/env python3
"""Compile real downstream crates; check diagnostic identities, not full snapshots."""
from pathlib import Path
import json
import os
import subprocess

root = Path(__file__).resolve().parents[1]
workspace = root.parents[1]
scratch = root / "target" / "boundary-consumer"
(scratch / "src").mkdir(parents=True, exist_ok=True)
manifest = '''[package]
name = "nitori-boundary-consumer"
version = "0.0.0"
edition = "2024"
[workspace]
[dependencies]
'''
dependencies = [
    ("nitori_call", workspace / "crates" / "nitori_call"),
    ("nitori-data-frame-codec-example", root),
    ("nitori_io", workspace / "crates" / "nitori_io"),
]
def consumer_manifest(include_example):
    text = manifest + 'futures-core = "0.3.32"\n'
    for name, path in dependencies:
        if include_example or name != "nitori-data-frame-codec-example":
            text += f'{name} = {{ path = {json.dumps(str(path))} }}\n'
    return text
(scratch / "Cargo.toml").write_text(consumer_manifest(True))
# Reuse exactly the validated versions from the owning workspace.
(scratch / "Cargo.lock").write_text((workspace / "Cargo.lock").read_text())
environment = dict(os.environ, CARGO_TARGET_DIR=str(root / "target" / "boundary-build"), CARGO_TERM_COLOR="never")
failures = []
for source in sorted((root / "tests" / "ui").glob("*.rs")):
    text = source.read_text()
    expected = text.splitlines()[0].removeprefix("// expect: ")
    # Facade-only probes depend on the libraries but not on this example crate.
    facade_only = "// facade-only" in text.splitlines()
    (scratch / "Cargo.toml").write_text(consumer_manifest(not facade_only))
    # Each consumer starts from the workspace's validated dependency versions.
    (scratch / "Cargo.lock").write_text((workspace / "Cargo.lock").read_text())
    (scratch / "src" / "main.rs").write_text(text)
    result = subprocess.run(
        ["cargo", "check", "--offline", "--quiet", "--manifest-path", str(scratch / "Cargo.toml")],
        cwd=workspace, env=environment, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    (scratch / f"{source.stem}.log").write_text(result.stdout)
    passed = (result.returncode == 0) if expected == "pass" else (result.returncode != 0 and expected in result.stdout)
    print(f'{"PASS" if passed else "FAIL"}: {source.name} ({expected})', flush=True)
    if not passed:
        failures.append(source.name)
        print(result.stdout)
if failures:
    raise SystemExit("unexpected boundary results: " + ", ".join(failures))
