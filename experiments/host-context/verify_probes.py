#!/usr/bin/env python3
"""Compile lending-boundary examples and check diagnostic categories."""

import json
from pathlib import Path
import shlex
import subprocess
import sys


ROOT = Path(__file__).resolve().parent
LOGS = ROOT / "target" / "probe-logs"
PROBES = (
    ("pass_lending_view", None, None),
    ("pass_transferred_parent_loan", None, None),
    ("pass_recursive_family_bridge", None, None),
    ("fail_view_escape", "E0515", None),
    ("fail_overlapping_borrows", "E0499", None),
    ("fail_cached_parent_projection", None, "lifetime may not live long enough"),
    ("fail_cached_borrowed_parent", None, "lifetime may not live long enough"),
    ("fail_move_pinned_view", "E0277", "cannot be unpinned"),
    ("fail_shorten_invariant_view", None, "lifetime may not live long enough"),
    ("fail_rebuild_mutex_guard", "E0507", None),
    ("fail_gat_family_bridge", None, "lifetime may not live long enough"),
    ("fail_fixed_coroutine_view", "E0277", None),
    ("fail_covariant_view_under_mut", None, "lifetime may not live long enough"),
    ("fail_generic_request_dispatch", "E0038", None),
)


def record_run(name, command):
    result = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    (LOGS / f"{name}.command.txt").write_text(shlex.join(command) + "\n")
    (LOGS / f"{name}.stdout").write_text(result.stdout)
    (LOGS / f"{name}.stderr").write_text(result.stderr)
    (LOGS / f"{name}.status.txt").write_text(f"{result.returncode}\n")
    return result


def diagnostics(stderr):
    for line in stderr.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("level") == "error":
            yield message


def main():
    LOGS.mkdir(parents=True, exist_ok=True)
    toolchain = record_run("toolchain", ["rustc", "+nightly", "--version", "--verbose"])
    if toolchain.returncode:
        print(f"FAIL toolchain; inspect {LOGS / 'toolchain.stderr'}", file=sys.stderr)
        return 1

    failures = []
    for name, error_code, fragment in PROBES:
        binary = LOGS / name
        result = record_run(
            name,
            [
                "rustc",
                "+nightly",
                "--edition=2024",
                "--error-format=json",
                str(ROOT / "probes" / f"{name}.rs"),
                "-o",
                str(binary),
            ],
        )
        if name.startswith("pass_"):
            ok = result.returncode == 0
            if ok:
                executed = record_run(f"{name}.run", [str(binary)])
                ok = executed.returncode == 0
        else:
            errors = [error for error in diagnostics(result.stderr) if error.get("spans")]
            matched = errors and all(
                (error_code is None or (error.get("code") or {}).get("code") == error_code)
                and (fragment is None or fragment in error["message"])
                and any(
                    span["is_primary"]
                    and Path(span["file_name"]).name == f"{name}.rs"
                    for span in error["spans"]
                )
                for error in errors
            )
            ok = result.returncode != 0 and matched

        print(f"{'PASS' if ok else 'FAIL'} {name}")
        if not ok:
            failures.append(name)

    print(f"Complete logs: {LOGS}")
    return bool(failures)


if __name__ == "__main__":
    sys.exit(main())
