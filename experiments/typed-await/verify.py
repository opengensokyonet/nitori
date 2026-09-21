#!/usr/bin/env python3
"""Reproduce the isolated prototype without modifying the source checkout."""
import argparse
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

BASE = "eefb694aa4ce6184c1d6658f09222dc76cff5b8f"
HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--snafu", type=Path, required=True)
parser.add_argument("--miri", action="store_true")
args = parser.parse_args()
snafu = args.snafu.resolve(strict=True)
destination = Path(tempfile.mkdtemp(prefix="nitori-typed-await-"))
print(f"Prototype checkout: {destination}", flush=True)
archive = subprocess.check_output(["git", "archive", BASE], cwd=REPO)
with tarfile.open(fileobj=io.BytesIO(archive)) as source:
    source.extractall(destination, filter="data")
subprocess.run(["git", "apply", str(HERE / "prototype.patch")], cwd=destination, check=True)
shutil.copyfile(HERE / "typed.rs", destination / "crates/nitori_call/src/typed.rs")
test_file = destination / "crates/nitori_call/tests/typed_children.rs"
shutil.copyfile(HERE / "typed_children.rs", test_file)
(destination / ".cargo").mkdir(exist_ok=True)
(destination / ".cargo/config.toml").write_text(
    "[patch.crates-io]\n"
    f"snafu = {{ path = {json.dumps(str(snafu))} }}\n"
    f"snafu-derive = {{ path = {json.dumps(str(snafu / 'snafu-derive'))} }}\n"
)


def cargo(*command, **kwargs):
    return subprocess.run(["cargo", "+nightly", *command], cwd=destination, check=True, **kwargs)


metadata = json.loads(cargo("metadata", "--locked", "--format-version=1", capture_output=True, text=True).stdout)
for name, folder in [("snafu", snafu), ("snafu-derive", snafu / "snafu-derive")]:
    matches = [p for p in metadata["packages"] if p["name"] == name]
    assert len(matches) == 1 and Path(matches[0]["manifest_path"]).parent == folder, matches
cargo("fmt", "--all", "--check")
cargo("test", "--locked", "--workspace", "--all-targets")
cargo("test", "--locked", "--workspace", "--all-features", "--all-targets")
cargo("test", "--locked", "--workspace", "--all-features", "--doc")
cargo("clippy", "--locked", "--workspace", "--all-features", "--all-targets", "--", "-D", "warnings")
for script in ["check_boundaries.py", "check_resume.py"]:
    subprocess.run(["python3", f"examples/data-frame-codec/scripts/{script}"], cwd=destination, check=True)
if args.miri:
    for flags in ["", "-Zmiri-tree-borrows"]:
        cargo("miri", "test", "--locked", "-p", "nitori_call", "--test", "typed_children",
              env={**os.environ, "MIRIFLAGS": flags})

negative_cases = {
    "unpinned": ("""
#[call]
async fn rejected(io: Receiver<'_, Host>) {
    let mut child = io.decode(0);
    child.next().await;
}
""", "no method named `next`"),
    "host-borrow-escape": ("""
#[call]
async fn rejected(io: Receiver<'_, Host>) -> usize {
    let borrowed = io.as_ref().get_ref();
    Pause(false).await;
    borrowed.count()
}
""", "lifetime may not live long enough"),
    "overlapping-next": ("""
#[call]
async fn rejected(io: Receiver<'_, Host>) {
    let mut child = pin!(io.decode(0));
    let first = child.as_mut().next();
    let second = child.as_mut().next();
    first.await;
    second.await;
}
""", "cannot borrow `child` as mutable more than once"),
    "known-return-child-limitation": ((HERE / "return-child.rs").read_text(), "hidden type of opaque"),
}
original = test_file.read_text()
try:
    for name, (snippet, expected) in negative_cases.items():
        test_file.write_text(original + "\n" + snippet)
        result = subprocess.run(
            ["cargo", "+nightly", "check", "--locked", "-p", "nitori_call", "--test", "typed_children"],
            cwd=destination, capture_output=True, text=True,
        )
        (destination / f"{name}.log").write_text(result.stdout + result.stderr)
        assert result.returncode != 0 and expected in result.stderr, (name, result.stderr)
        print(f"Verified rejection: {name}", flush=True)
finally:
    test_file.write_text(original)
print("Prototype verification passed; return-child limitation reproduced.")
