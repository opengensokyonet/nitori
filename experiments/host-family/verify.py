#!/usr/bin/env python3
"""Check the lending design, safety rejections, and rejected alternative designs."""
import argparse
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent


def run(command, *, env=None):
    subprocess.run(command, cwd=ROOT, env=env, check=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--miri", action="store_true")
    args = parser.parse_args()
    output = Path(tempfile.mkdtemp(prefix="nitori-host-family-"))
    print(f"Compiler probes and diagnostics: {output}", flush=True)
    run(["cargo", "+nightly", "fmt", "--", "--check"])
    run(["cargo", "+nightly", "clippy", "--offline", "--all-targets", "--", "-D", "warnings"])
    run(["cargo", "+nightly", "test", "--offline"])
    library = output / "libhost_family.rlib"
    run(["rustc", "+nightly", "--edition=2024", "--crate-type=rlib", "--crate-name=host_family",
         "src/lib.rs", "-o", str(library)])
    cases = [
        ("nested_fixed", "pass", None),
        ("with_fixed", "pass", None),
        ("with_nominal_borrowed", "pass", None),
        ("escape", "safety rejection", "outlive `'static`"),
        ("store_host", "safety rejection", "lifetime may not live long enough"),
        ("with_nominal_escape", "safety rejection", "lifetime may not live long enough"),
        ("with_nominal_store", "safety rejection", "borrowed data escapes outside of closure"),
        ("nested_compose", "rejected alternative", "invariant"),
        ("nested_concrete", "rejected alternative", "invariant"),
        ("with_borrowed", "rejected alternative", "implies a `'static` lifetime"),
    ]
    for name, kind, diagnostic in cases:
        result = subprocess.run(
            ["rustc", "+nightly", "--edition=2024", f"probes/{name}.rs", "--extern",
             f"host_family={library}", "-o", str(output / name)],
            cwd=ROOT, text=True, capture_output=True,
        )
        (output / f"{name}.stderr").write_text(result.stderr)
        if kind == "pass":
            if result.returncode:
                raise RuntimeError(result.stderr)
            run([str(output / name)])
        elif result.returncode == 0 or diagnostic not in result.stderr or "error[E0658]" in result.stderr:
            raise RuntimeError(f"unexpected diagnostic for {name}:\n{result.stderr}")
        print(f"{kind}: {name}", flush=True)
    if args.miri:
        run(["cargo", "+nightly", "miri", "test", "--offline", "--tests"])
        env = os.environ.copy()
        env["MIRIFLAGS"] = "-Zmiri-tree-borrows"
        run(["cargo", "+nightly", "miri", "test", "--offline", "--tests"], env=env)
    print("Family execution, explicit reborrowing, and nominal with callbacks passed the checked boundaries.")


if __name__ == "__main__":
    main()
