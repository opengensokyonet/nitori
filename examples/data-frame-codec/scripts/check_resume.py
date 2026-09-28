#!/usr/bin/env python3
"""Check direct-reference resume with default and alternative compiler settings."""
from pathlib import Path
import subprocess
root=Path(__file__).resolve().parents[1]
output=root/'target/resume-probes';output.mkdir(parents=True,exist_ok=True)
settings={'default':[], 'next-solver':['-Znext-solver=globally'], 'polonius':['-Zpolonius=next']}
failures=[]
for source in sorted((root/'tests/resume').glob('*.rs')):
    expected=source.read_text().splitlines()[0].removeprefix('// expect: ')
    for mode,flags in settings.items():
        binary=output/f'{source.stem}-{mode}'
        result=subprocess.run(['rustc','--edition=2024',str(source),'-o',str(binary),*flags],cwd=root,text=True,capture_output=True)
        log=result.stdout+result.stderr
        passed=(result.returncode==0) if expected=='pass' else (result.returncode!=0 and expected in log)
        if passed and expected=='pass':
            run=subprocess.run([str(binary)],text=True,capture_output=True)
            log+=run.stdout+run.stderr;passed=run.returncode==0
        (output/f'{source.stem}-{mode}.log').write_text(log)
        print(f'{"PASS" if passed else "FAIL"}: {source.name} [{mode}] ({expected})',flush=True)
        if not passed:failures.append((source.name,mode));print(log)
if failures:raise SystemExit(str(failures))
