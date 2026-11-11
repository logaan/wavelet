#!/usr/bin/env python3
import argparse
import collections
import json
import os
import venv
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
BASELINE = ROOT / 'quality-baseline.json'


def run(*args):
    return subprocess.run(args, cwd=ROOT, text=True, capture_output=True)


def analyzer():
    try:
        import lizard
        return lizard
    except ImportError:
        directory = ROOT / 'target' / 'quality-venv'
        venv.create(directory, with_pip=True)
        python = directory / ('Scripts/python.exe' if os.name == 'nt' else 'bin/python')
        subprocess.run([str(python), '-m', 'pip', 'install', '-r',
                        str(ROOT / 'scripts/quality-requirements.txt')], check=True)
        os.execv(str(python), [str(python), str(Path(__file__).resolve()), *sys.argv[1:]])


def measure():
    lizard = analyzer()
    result = run('cargo', 'clippy', '--all-targets', '--message-format=json', '--',
                 '-W', 'clippy::cognitive_complexity', '-W', 'clippy::too_many_lines')
    diagnostics = {}
    for line in result.stdout.splitlines():
        try:
            item = json.loads(line)
        except ValueError:
            continue
        if item.get('reason') != 'compiler-message':
            continue
        message = item['message']
        if message['level'] not in ('warning', 'error'):
            continue
        code = (message.get('code') or {}).get('code', message['level'])
        spans = message.get('spans', [])
        span = next((span for span in spans if span.get('is_primary')), {})
        key = (code, span.get('file_name', ''), span.get('line_start', 0), message['message'])
        diagnostics[key] = message.get('rendered', message['message'])
    if result.returncode:
        print(result.stderr, file=sys.stderr)
        for text in diagnostics.values():
            print(text, file=sys.stderr)
        raise SystemExit(result.returncode)
    warnings = collections.Counter(key[0] for key in diagnostics)
    sizes = {str(path.relative_to(ROOT)): len(path.read_text().splitlines())
             for path in (ROOT / 'src').rglob('*.rs')}
    functions = {}
    complexity = {}
    for path in (ROOT / 'src').rglob('*.rs'):
        for function in lizard.analyze_file(str(path)).function_list:
            name = f'{path.relative_to(ROOT)}::{function.name}'
            if function.nloc > 200:
                functions[name] = max(functions.get(name, 0), function.nloc)
            if function.cyclomatic_complexity > 40:
                complexity[name] = max(complexity.get(name, 0), function.cyclomatic_complexity)
    return {'function_lines': dict(sorted(functions.items())),
            'cyclomatic_complexity': dict(sorted(complexity.items())),
            'warnings': dict(sorted(warnings.items())),
            'large_modules': dict(sorted((name, size) for name, size in sizes.items() if size > 1000))}, diagnostics


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--update-baseline', action='store_true')
    args = parser.parse_args()
    formatted = run('cargo', 'fmt', '--check')
    if formatted.returncode:
        print(formatted.stdout + formatted.stderr, file=sys.stderr)
        return formatted.returncode
    current, diagnostics = measure()
    if args.update_baseline:
        BASELINE.write_text(json.dumps(current, indent=2) + '\n')
        print('Updated quality baseline; review the diff before committing.')
        return 0
    baseline = json.loads(BASELINE.read_text())
    failures = []
    for category, items in current.items():
        for name, count in items.items():
            defaults = {'large_modules': 1000, 'function_lines': 200, 'cyclomatic_complexity': 40, 'warnings': 0}
            limit = baseline[category].get(name, defaults[category])
            if category == 'function_lines':
                limit = min(limit, 999)
            if count > limit:
                failures.append(f'{category}: {name}: {count} exceeds {limit}')
    for failure in failures:
        print(failure, file=sys.stderr)
    if failures:
        for text in diagnostics.values():
            print(text, file=sys.stderr)
        return 1
    print('Formatting, Clippy, complexity and module-size ratchets passed.')
    return 0


if __name__ == '__main__':
    sys.exit(main())
