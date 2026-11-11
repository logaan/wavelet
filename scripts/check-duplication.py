#!/usr/bin/env python3
import argparse
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
PACKAGE = ROOT / 'tooling/quality'
BASELINE = ROOT / 'quality-duplication.json'
REPORT = ROOT / 'target/duplication/jscpd-report.json'


def detector():
    expected = json.loads((PACKAGE / 'package.json').read_text())['devDependencies']['jscpd']
    installed = PACKAGE / 'node_modules/jscpd/package.json'
    if not installed.exists() or json.loads(installed.read_text())['version'] != expected:
        subprocess.run(['npm', 'ci', '--prefix', str(PACKAGE)], cwd=ROOT, check=True)
    return PACKAGE / 'node_modules/jscpd/run-jscpd.js'


def location(fragment):
    path = Path(fragment['name']).relative_to(ROOT)
    return f"{path}:{fragment['start']}"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--update-baseline', action='store_true')
    args = parser.parse_args()
    if not args.update_baseline and not BASELINE.is_file():
        print(f'Missing duplication baseline: {BASELINE}', file=sys.stderr)
        return 1
    REPORT.unlink(missing_ok=True)
    command = ['node', str(detector()), '--config', '.jscpd.json', '--absolute',
               '--baseline', str(BASELINE)]
    command.append('--update-baseline' if args.update_baseline else '--fail-on-new-clones')
    result = subprocess.run(command, cwd=ROOT)
    if not REPORT.is_file():
        return result.returncode or 1
    report = json.loads(REPORT.read_text())
    totals = report['statistics']['total']
    print(f"Duplication: {totals['clones']} clones, {totals['duplicatedLines']} duplicated "
          f"lines ({totals['percentage']:.2f}%), {totals['sources']} source files.")
    if args.update_baseline:
        print('Updated duplication baseline; review removed and accepted clones before committing.')
    else:
        for clone in report['duplicates']:
            if clone['isNew']:
                print(f"New {clone['format']} clone ({clone['lines']} lines): "
                      f"{location(clone['firstFile'])} and {location(clone['secondFile'])}",
                      file=sys.stderr)
    print(f'Report: {REPORT.relative_to(ROOT)}')
    return result.returncode


if __name__ == '__main__':
    sys.exit(main())
