#!/usr/bin/env python3
import argparse
import csv
from datetime import date, timedelta
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
CONFIG = ROOT / 'tooling/quality/code-maat.json'
CACHE = ROOT / 'target/code-maat'
SCHEMAS = {
    'revisions': ['entity', 'n-revs'],
    'entity-churn': ['entity', 'added', 'deleted', 'commits'],
    'coupling': ['entity', 'coupled', 'degree', 'average-revs'],
}


def run(command, cwd=ROOT):
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(f"{' '.join(map(str, command))} failed:\n"
                           f"{result.stderr or result.stdout}")
    return result.stdout


def java_runtime():
    home = os.environ.get('JAVA_HOME')
    candidates = [str(Path(home) / 'bin/java')] if home else ['java']
    if not home and shutil.which('brew'):
        prefix = subprocess.run(['brew', '--prefix', 'openjdk@17'], text=True,
                                capture_output=True)
        if prefix.returncode == 0:
            candidates.insert(0, str(Path(prefix.stdout.strip()) / 'bin/java'))
    errors = []
    for candidate in candidates:
        try:
            result = subprocess.run([candidate, '-version'], text=True, capture_output=True)
            if result.returncode == 0:
                return candidate, (result.stderr or result.stdout).strip()
            errors.append(result.stderr.strip())
        except OSError as error:
            errors.append(str(error))
    raise RuntimeError('Java is required for Code Maat. Install Java 17 and set JAVA_HOME '
                       '(macOS: brew install openjdk@17).\n' + '\n'.join(errors))


def verify_jar(path, expected):
    actual = hashlib.sha256(path.read_bytes()).hexdigest()
    if actual != expected:
        raise RuntimeError(f'Code Maat checksum mismatch for {path}: expected {expected}, '
                           f'got {actual}. Remove the cached JAR and retry.')


def install_jar(config):
    CACHE.mkdir(parents=True, exist_ok=True)
    filename = f"code-maat-{config['version']}-standalone.jar"
    jar = CACHE / filename
    if jar.exists():
        verify_jar(jar, config['sha256'])
        return jar
    url = f"https://github.com/adamtornhill/code-maat/releases/download/v{config['version']}/{filename}"
    with tempfile.TemporaryDirectory(prefix='download-', dir=CACHE) as temporary:
        downloaded = Path(temporary) / filename
        print(f'Downloading pinned Code Maat {config["version"]}...', flush=True)
        run(['curl', '--fail', '--location', '--retry', '3', '--connect-timeout', '20',
             '--max-time', '180', '--output', str(downloaded), url])
        verify_jar(downloaded, config['sha256'])
        downloaded.replace(jar)
    return jar


def history(repository, since, limit, paths):
    if run(['git', 'rev-parse', '--is-shallow-repository'], repository).strip() == 'true':
        raise RuntimeError('Code Maat requires complete history. Run git fetch --unshallow '
                           'or use actions/checkout with fetch-depth: 0.')
    head = run(['git', 'rev-parse', 'HEAD'], repository).strip()
    filters = ['--no-merges', f'--since={since}T00:00:00Z', head]
    commits = run(['git', 'rev-list', f'--max-count={limit + 1}', *filters, '--', *paths],
                  repository).splitlines()
    log = run(['git', '-c', 'core.quotepath=false', 'log', f'--max-count={limit}',
               '--numstat', '--date=short', '--pretty=format:--%H--%ad--history', '--no-renames',
               *filters, '--', *paths], repository)
    return head, log, min(len(commits), limit), len(commits) > limit


def analyze(java, jar, log, directory, config, empty=False):
    results = {}
    for analysis, expected in SCHEMAS.items():
        command = [java, '-Xmx1g', '-Djava.awt.headless=true', '-jar', str(jar),
                   '-l', str(log), '-c', 'git2', '-a', analysis]
        if analysis == 'coupling':
            for name, value in config['coupling'].items():
                command.extend([f'--{name}', str(value)])
        output = ','.join(expected) + '\n' if empty else run(command)
        reader = csv.DictReader(io.StringIO(output))
        if reader.fieldnames != expected:
            raise RuntimeError(f'Unexpected Code Maat {analysis} output:\n{output[:1000]}')
        results[analysis] = list(reader)
        (directory / f'{analysis}.csv').write_text(output, encoding='utf-8')
    return results


def hotspots(repository, results, directory):
    churn = {row['entity']: row for row in results['entity-churn']}
    rows = []
    for revision in results['revisions']:
        entity = revision['entity']
        path = repository / entity
        if not path.is_file() or path.is_symlink():
            continue
        changes = churn.get(entity, {})
        rows.append({'entity': entity, 'revisions': int(revision['n-revs']),
                     'added': int(changes.get('added', 0)),
                     'deleted': int(changes.get('deleted', 0)),
                     'current-lines': len(path.read_bytes().splitlines())})
    rows.sort(key=lambda row: (-row['revisions'], -(row['added'] + row['deleted']), row['entity']))
    with (directory / 'hotspots.csv').open('w', newline='', encoding='utf-8') as output:
        writer = csv.DictWriter(output, fieldnames=['entity', 'revisions', 'added', 'deleted',
                                                   'current-lines'])
        writer.writeheader()
        writer.writerows(rows)
    return rows


def summary(metadata, rows, coupling):
    text = ['# Code Maat history report', '',
            f"Commit: `{metadata['head']}`; since {metadata['since']}; "
            f"{metadata['commits']} non-merge commits analyzed.", '',
            f"History capped at {metadata['max-commits']} commits: {metadata['history-capped']}.",
            '', 'Advisory evidence for review; churn and co-change are not correctness failures.',
            'Current file sizes include uncommitted changes; revision and churn data stop at HEAD.',
            '', '## Frequently changed current files', '',
            '| File | Revisions | Added | Deleted | Current lines |',
            '| --- | ---: | ---: | ---: | ---: |']
    for row in rows[:20]:
        text.append(f"| {row['entity']} | {row['revisions']} | {row['added']} | "
                    f"{row['deleted']} | {row['current-lines']} |")
    text.extend(['', '## Files that changed together', '',
                 f"Filters: `{json.dumps(metadata['coupling'], sort_keys=True)}`.", '',
                 '| File | Coupled file | Degree (%) | Average revisions |',
                 '| --- | --- | ---: | ---: |'])
    for row in coupling[:20]:
        text.append(f"| {row['entity']} | {row['coupled']} | {row['degree']} | {row['average-revs']} |")
    if not coupling:
        text.extend(['', 'No pairs met the configured evidence filters.'])
    return '\n'.join(text) + '\n'


def report(repository, since, limit, config, java, java_version, jar):
    head, log, count, capped = history(repository, since, limit, config['paths'])
    destination = repository / 'target/code-maat/reports'
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='report-', dir=destination.parent) as temporary:
        directory = Path(temporary)
        log_path = directory / 'history.log'
        log_path.write_text(log, encoding='utf-8')
        results = analyze(java, jar, log_path, directory, config, empty=count == 0)
        rows = hotspots(repository, results, directory)
        metadata = {'head': head, 'since': since, 'commits': count, 'max-commits': limit,
                    'history-capped': capped, 'tool-version': config['version'],
                    'jar-sha256': config['sha256'], 'java': java_version,
                    'coupling': config['coupling'], 'paths': config['paths']}
        (directory / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
        (directory / 'summary.md').write_text(summary(metadata, rows, results['coupling']))
        destination.mkdir(exist_ok=True)
        for path in directory.iterdir():
            path.replace(destination / path.name)
    print(f'Code Maat: {count} commits, {len(rows)} current files, '
          f"{len(results['coupling'])} coupling pairs; history capped: {capped}.")
    print(f'Advisory report: {destination / "summary.md"}')
    return destination


def main():
    config = json.loads(CONFIG.read_text())
    parser = argparse.ArgumentParser(description='Report Code Maat history metrics without metric thresholds.')
    parser.add_argument('--repo', type=Path, default=ROOT)
    parser.add_argument('--since', type=date.fromisoformat)
    parser.add_argument('--max-commits', type=int, default=config['maxCommits'])
    args = parser.parse_args()
    if args.max_commits < 1:
        parser.error('--max-commits must be positive')
    repository = args.repo.resolve()
    head_date = date.fromisoformat(run(['git', 'show', '-s', '--format=%cs', 'HEAD'], repository).strip())
    since = args.since or head_date - timedelta(days=config['historyDays'])
    java, java_version = java_runtime()
    jar = install_jar(config)
    report(repository, since.isoformat(), args.max_commits, config, java, java_version, jar)
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, RuntimeError, ValueError) as error:
        print(f'Code Maat failed: {error}', file=sys.stderr)
        sys.exit(1)
