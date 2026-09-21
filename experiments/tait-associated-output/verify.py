#!/usr/bin/env python3
"""Check the TAIT reproductions and controls without any Cargo dependencies."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent
output = Path(tempfile.mkdtemp(prefix="tait-associated-output-"))
print(f"Diagnostics: {output}", flush=True)
for solver, flags in [("default", []), ("next", ["-Znext-solver=globally"])]:
    for name, succeeds in [("minimal", False), ("coroutine", False), ("nominal", True), ("rpit", True)]:
        binary = output / f"{name}-{solver}"
        result = subprocess.run(
            ["rustc", "+nightly", "--edition=2024", *flags, str(root / f"{name}.rs"), "-o", str(binary)],
            text=True, capture_output=True,
        )
        (output / f"{name}-{solver}.log").write_text(result.stdout + result.stderr)
        if succeeds:
            assert result.returncode == 0, result.stderr
            subprocess.run([str(binary)], check=True)
        else:
            assert result.returncode != 0 and "hidden type of opaque" in result.stderr, result.stderr
        print(f"PASS: {name} [{solver}], expected {'success' if succeeds else 'E0282'}")
