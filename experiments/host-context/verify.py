#!/usr/bin/env python3
"""Run the context experiment's checks with complete per-command logs."""

from pathlib import Path
import shlex
import subprocess
import sys


ROOT = Path(__file__).resolve().parent
LOGS = ROOT / "target" / "verification-logs"


def main():
    LOGS.mkdir(parents=True, exist_ok=True)
    commands = [
        ["cargo", "+nightly", "fmt", "--all", "--check"],
        ["rustfmt", "+nightly", "--edition", "2024", "--check",
         *[str(path) for path in sorted((ROOT / "probes").glob("*.rs"))]],
        ["cargo", "+nightly", "clippy", "--locked", "--offline", "--all-targets", "--", "-D", "warnings"],
        ["cargo", "+nightly", "test", "--locked", "--offline", "--all-targets"],
        ["cargo", "+nightly", "test", "--locked", "--offline", "--doc"],
        [sys.executable, "verify_probes.py"],
    ]
    for index, command in enumerate(commands):
        prefix = LOGS / f"{index:02}"
        prefix.with_suffix(".command.txt").write_text(shlex.join(command) + "\n")
        with prefix.with_suffix(".log").open("w") as output:
            result = subprocess.run(command, cwd=ROOT, stdout=output, stderr=subprocess.STDOUT)
        prefix.with_suffix(".status.txt").write_text(f"{result.returncode}\n")
        print(f"{'PASS' if result.returncode == 0 else 'FAIL'} {shlex.join(command)}", flush=True)
        if result.returncode:
            print(f"Inspect {prefix.with_suffix('.log')}", file=sys.stderr)
            return result.returncode
    print(f"Complete logs: {LOGS}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
