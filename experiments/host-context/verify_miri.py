#!/usr/bin/env python3
"""Run all runtime tests under Miri's two aliasing models, retaining each run."""

from datetime import datetime, timezone
import os
from pathlib import Path
import shlex
import subprocess
import sys


ROOT = Path(__file__).resolve().parent
LOGS = ROOT / "target" / "miri-logs"


def run(logs, name, command, flags):
    environment = os.environ.copy()
    # Keep the checks reproducible even when the invoking shell has MIRIFLAGS.
    # Neither run suppresses isolation, validity, alignment, or leak checks.
    environment["MIRIFLAGS"] = flags
    prefix = logs / name
    description = f"MIRIFLAGS={shlex.quote(flags)} {shlex.join(command)}"
    prefix.with_suffix(".command.txt").write_text(description + "\n")
    print(f"RUN {description}", flush=True)
    with prefix.with_suffix(".log").open("w") as output:
        try:
            result = subprocess.run(
                command, cwd=ROOT, env=environment,
                stdout=output, stderr=subprocess.STDOUT,
            )
            status = result.returncode
        except OSError as error:
            output.write(f"Failed to execute command: {error}\n")
            status = 127
    prefix.with_suffix(".status.txt").write_text(f"{status}\n")
    print(f"{'PASS' if status == 0 else 'FAIL'} {name}: {prefix.with_suffix('.log')}", flush=True)
    return status


def main():
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    logs = LOGS / stamp
    logs.mkdir(parents=True)
    print(f"Complete logs: {logs}", flush=True)
    for name, command in [
        ("rustc-version", ["rustc", "+nightly", "--version", "--verbose"]),
        ("miri-version", ["cargo", "+nightly", "miri", "--version"]),
    ]:
        status = run(logs, name, command, "")
        if status:
            return status

    command = ["cargo", "+nightly", "miri", "test", "--locked", "--offline", "--all-targets"]
    statuses = [
        run(logs, "stacked-borrows", command, ""),
        run(logs, "tree-borrows", command, "-Zmiri-tree-borrows"),
    ]
    # Run both models even after one fails, preserving the comparison evidence.
    return next((status for status in statuses if status), 0)


if __name__ == "__main__":
    sys.exit(main())
