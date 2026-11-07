#!/usr/bin/env python3
"""Differential tests of kusd (examples/kusd.rs) against OpenUSD's usdcat
over OpenUSD's own test corpus (every testenv layer). Local only: OpenUSD
is an oracle, never a dependency.

Suites:
  usda        each .usda read and printed by kusd: byte-identical to usdcat
              ('semantic' when usdcat reads ours back to the same text)
  usdc        each .usdc read and printed as usda: identical to usdcat
  usdz        each .usdz's root layer printed as usda: identical to usdcat
  usdc-write  each .usda written as .usdc by kusd, read back by usdcat
  usda-write  each .usdc written as .usda by kusd, read back by usdcat
  flatten     each .usda composed and flattened: as usdcat --flatten
  scene       each layer's scene read and written by kusd: usdcat must
              read every written layer
Write suites count a layer both tools refuse to write as agreeing.

Environment: OPENUSD (an OpenUSD v26.08 checkout, required), USD_BIN (the
bin directory of its build, holding usdcat; else usdcat on PATH),
PARITY_CACHE (usdcat output cache).

Usage: cargo build --release --example kusd
       tools/parity.py <target>/release/examples/kusd [suite ...] [--failures FILE]
"""
import argparse
import hashlib
import os
import re
import subprocess
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

OPENUSD = Path(os.environ['OPENUSD']) if 'OPENUSD' in os.environ else None
USDCAT = str(Path(os.environ['USD_BIN']) / 'usdcat') if 'USD_BIN' in os.environ else 'usdcat'
CACHE = Path(os.environ.get('PARITY_CACHE', Path(tempfile.gettempdir()) / 'kusd-parity-cache'))
ENV = {**os.environ, 'USD_ASSIGN_PROTOTYPES_DETERMINISTICALLY': '1', 'PXR_USDC_EMIT_DEPRECATION_WARNINGS': '0'}
DOC = re.compile(rb'    doc = """Generated from Composed Stage of root layer [^\n]*\n"""\n')


def corpus(suffix):
    return sorted(p for p in (OPENUSD / 'pxr').rglob(f'*{suffix}') if 'testenv' in str(p))


def run(command, timeout=120):
    try:
        result = subprocess.run(command, capture_output=True, timeout=timeout, env=ENV)
        return result.returncode, result.stdout, result.stderr
    except subprocess.TimeoutExpired:
        return -1, b'', b'timeout'


def usdcat(path, *extra, cache=True):
    """usdcat's usda text for `path`, or None when usdcat refuses it."""
    key = hashlib.sha1(f'{path}|{Path(path).stat().st_mtime_ns}|{extra}'.encode()).hexdigest()
    cached = CACHE / key
    if cache and cached.exists():
        data = cached.read_bytes()
        return None if data == b'\0R' else data
    code, out, _ = run([USDCAT, *extra, str(path), '--usdFormat', 'usda'])
    data = out if code == 0 else None
    if cache:
        CACHE.mkdir(parents=True, exist_ok=True)
        cached.write_bytes(data if data is not None else b'\0R')
    return data


def usdcat_writes(path, extension, scratch):
    """Whether usdcat itself can write `path` as `extension`."""
    out = Path(scratch) / ('oracle-' + hashlib.sha1(str(path).encode()).hexdigest() + extension)
    code, _, _ = run([USDCAT, str(path), '-o', str(out)])
    return code == 0


def check(kusd, mode, path, scratch):
    if mode == 'scene':
        out = Path(scratch) / (hashlib.sha1(str(path).encode()).hexdigest() + '.usda')
        code, _, err = run([kusd, 'scene', str(path), str(out)])
        if code != 0:
            return ('refused' if code == 1 else 'crash', err[-300:])
        return ('ok', None) if usdcat(out, cache=False) is not None else ('usdcat-rejects', None)
    expected = usdcat(path, '--flatten') if mode == 'flatten' else usdcat(path)
    if expected is None:
        return None
    raw = expected
    if mode == 'flatten':
        expected = DOC.sub(b'', expected).replace(b'#usda 1.0\n(\n)\n', b'#usda 1.0\n')
    if mode in ('cat', 'flatten'):
        code, out, err = run([kusd, mode, str(path)])
        if code != 0:
            return ('fail', err[-300:])
        if out in (expected, raw):
            return ('bytes', None)
        written = Path(scratch) / (hashlib.sha1(str(path).encode()).hexdigest() + '.usda')
        written.write_bytes(out)
        return ('semantic', None) if usdcat(written, cache=False) in (expected, raw) else ('differs', None)
    extension = '.' + mode
    out = Path(scratch) / (hashlib.sha1(str(path).encode()).hexdigest() + extension)
    code, _, err = run([kusd, 'write', str(path), str(out)])
    if code != 0:
        return ('both-refuse', None) if not usdcat_writes(path, extension, scratch) else ('fail', err[-300:])
    return ('semantic', None) if usdcat(out, cache=False) == expected else ('differs', None)


SUITES = {
    'usda': ('.usda', 'cat'),
    'usdc': ('.usdc', 'cat'),
    'usdz': ('.usdz', 'cat'),
    'usdc-write': ('.usda', 'usdc'),
    'usda-write': ('.usdc', 'usda'),
    'flatten': ('.usda', 'flatten'),
    'scene': (None, 'scene'),
}
GOOD = ('bytes', 'semantic', 'both-refuse', 'ok', 'refused')


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('kusd')
    parser.add_argument('suites', nargs='*', default=list(SUITES))
    parser.add_argument('--failures', type=Path)
    args = parser.parse_args()
    if OPENUSD is None:
        parser.error('set OPENUSD to an OpenUSD checkout (its testenv layers are the corpus)')
    failures = []
    for suite in args.suites:
        suffix, mode = SUITES[suite]
        files = corpus(suffix) if suffix else corpus('.usda') + corpus('.usdc') + corpus('.usdz')
        counts = {}
        with tempfile.TemporaryDirectory() as scratch, ThreadPoolExecutor(os.cpu_count() or 4) as pool:
            for path, result in zip(files, pool.map(lambda p: check(args.kusd, mode, p, scratch), files)):
                if result is None:
                    continue
                counts[result[0]] = counts.get(result[0], 0) + 1
                if result[0] not in GOOD:
                    failures.append(f'{suite}\t{result[0]}\t{path}\t{result[1]!r}')
        agreeing = sum(count for kind, count in counts.items() if kind in GOOD)
        print(f'{suite}: {agreeing}/{sum(counts.values())} agree {dict(sorted(counts.items()))}', flush=True)
    if args.failures:
        args.failures.write_text('\n'.join(failures) + '\n')


if __name__ == '__main__':
    main()
