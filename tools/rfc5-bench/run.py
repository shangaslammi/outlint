#!/usr/bin/env python3
"""Linux/glibc benchmark: run.py CHECKOUT OUTPUT.csv [--target-dir DIR].

Builds locked production Rust, then a standalone shell around unchanged private
sequence/JSON modules and the public parser/validator. No repository files are
modified. Native timing (5 processes x 5 calls) is separate from allocation
profiling (1 process x 1 call). Allocation counts/peak include input setup and
runtime startup; times include only the named operation and dropping its result.
"""
import argparse
import csv
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('checkout', type=Path)
parser.add_argument('output', type=Path)
parser.add_argument('--target-dir', type=Path)
parser.add_argument('--smoke', action='store_true', help='exercise every mode with tiny inputs')
args = parser.parse_args()
root = args.checkout.resolve()
target = (args.target_dir or root / 'target').resolve()
env = dict(os.environ, CARGO_TARGET_DIR=str(target))
build = subprocess.run(
    ['cargo', 'build', '--release', '--workspace', '--locked', '--message-format=json'],
    cwd=root, env=env, check=True, text=True, stdout=subprocess.PIPE,
)
libraries = {}
for line in build.stdout.splitlines():
    artifact = json.loads(line)
    if artifact.get('reason') != 'compiler-artifact':
        continue
    name = artifact['target']['name']
    if name not in ['outlint_core', 'serde_json']:
        continue
    if name == 'serde_json' and 'arbitrary_precision' not in artifact['features']:
        continue
    libraries[name] = next(Path(p) for p in artifact['filenames'] if p.endswith('.rlib'))
here = Path(__file__).resolve().parent
with tempfile.TemporaryDirectory(prefix='outlint-measure-') as tmp:
    tmp = Path(tmp)
    source = tmp / 'bench.rs'
    source.write_text((here / 'bench.rs.in').read_text().replace('@ROOT@', str(root)))
    command = ['rustc', '--edition=2021', '-O', str(source), '-L', f'dependency={target}/release/deps', '-o', str(tmp / 'bench')]
    for crate in ['outlint_core', 'serde_json']:
        library = libraries[crate]
        command += ['--extern', f'{crate}={library}']
    subprocess.run(command, check=True)
    subprocess.run(['cc', '-O2', '-shared', '-fPIC', str(here / 'alloc.c'), '-o', str(tmp / 'alloc.so')], check=True)
    subprocess.run([str(tmp / 'bench'), 'sizes', '0', '0', '1'], check=True)
    cases = [('parse', n, 0) for n in [100, 1000, 10000]]
    cases += [(mode, n, r) for mode in ['headings', 'sequence', 'sequence-recovery'] for n, r in [(128,128), (1024,128), (4096,128), (128,1024), (128,4096)]]
    cases += [('json', n, 0) for n in [100,1000,10000]]
    if args.smoke:
        cases = [(mode, 4, 4) for mode in ['parse', 'headings', 'sequence', 'sequence-recovery', 'json']]
    with args.output.open('w') as f:
        writer = csv.writer(f, lineterminator="\n")
        writer.writerow(['mode','nodes','rules','median_ns','min_ns','max_ns','alloc_calls','peak_usable_bytes'])
        for mode,n,r in cases:
            command = [str(tmp / 'bench'), mode, str(n), str(r)]
            times = [int(subprocess.check_output(command + ['5'], text=True)) for _ in range(5)]
            profile = subprocess.run(command + ['1'], env=dict(os.environ, LD_PRELOAD=str(tmp / 'alloc.so')), text=True, capture_output=True, check=True)
            calls, peak = re.search(r'alloc_calls=(\d+) peak_usable_bytes=(\d+)', profile.stderr).groups()
            row = [mode,n,r,int(statistics.median(times)),min(times),max(times),calls,peak]
            writer.writerow(row)
            f.flush()
            print(*row, flush=True)
