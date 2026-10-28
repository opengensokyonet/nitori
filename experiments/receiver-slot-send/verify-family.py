#!/usr/bin/env python3
"""Verify the receiver-slot experiment; write results under target/verification."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent
output_dir = root / "target" / "verification"
output_dir.mkdir(parents=True, exist_ok=True)
results = []

def run(name, args, *, expected=None, env=None, cwd=root):
    log = output_dir / (name + '.log')
    with log.open('w') as output:
        result = subprocess.run(args, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT)
    text = log.read_text()
    passed = result.returncode == 0 if expected is None else result.returncode != 0 and expected in text
    record = dict(name=name, exit_code=result.returncode, passed=passed, log=str(log))
    results.append(record)
    print(json.dumps(record), flush=True)
    if not passed:
        raise SystemExit(1)

cargo = ['cargo', '+nightly-2026-09-20']
run('family-format-final', ['rustup', 'run', 'nightly-2026-09-20', 'rustfmt', '--edition', '2024', '--check', 'family.rs'])
run('family-native-final', cargo + ['test', '--offline', '--test', 'family', '--test', 'scoped'])
run('family-miri-final', ['cargo', '+nightly', 'miri', 'test', '--offline', '--test', 'family'])
run('family-tree-final', ['cargo', '+nightly', 'miri', 'test', '--offline', '--test', 'family'], env={**os.environ, 'MIRIFLAGS': '-Zmiri-tree-borrows'})
for name, expected in {
    'rc': 'future cannot be sent between threads safely',
    'return': 'lifetime may not live long enough',
    'escape': 'lifetime may not live long enough',
    'view': 'lifetime may not live long enough',
    'sync': 'cannot be shared between threads safely',
    'send_loan': 'cannot be sent between threads safely',
}.items():
    run('family-reject-final-' + name, cargo + ['rustc', '--offline', '--test', 'family', '--', '--cfg', 'reject_' + name], expected=expected)
# The operation that defeated nitori_call's former probe-based Send check
# must now make its call not Send.
run('probe-counterexample-rejected', cargo + ['build', '--offline'],
    expected='cannot be sent between threads safely', cwd=root / 'probe-counterexample')
(output_dir / 'family-verification.json').write_text(json.dumps({
    'source_sha256': hashlib.sha256((root / 'family.rs').read_bytes()).hexdigest(),
    'results': results,
}, indent=2) + '\n')
