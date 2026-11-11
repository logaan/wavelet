import csv
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('code_maat', Path(__file__).parents[1] / 'code-maat.py')
MAAT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MAAT)


class HistoryAnalysisTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='wavelet-history-test-')
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name) / 'repository'
        self.repo.mkdir()
        self.git('init', '-q')
        self.config = json.loads(MAAT.CONFIG.read_text())
        self.config['coupling'].update({'min-revs': 2, 'min-shared-revs': 2})
        for number in range(1, 4):
            self.write('src/a.rs', 'a\n' * number)
            if number < 3:
                self.write('src/b.rs', 'b\n' * number)
            self.write('docs/src/wasm/generated.js', 'generated\n' * number)
            self.git('add', '.')
            environment = dict(os.environ, GIT_AUTHOR_NAME='Fixture', GIT_COMMITTER_NAME='Fixture',
                               GIT_AUTHOR_EMAIL='fixture@example.invalid',
                               GIT_COMMITTER_EMAIL='fixture@example.invalid',
                               GIT_AUTHOR_DATE=f'2026-01-0{number}T12:00:00Z',
                               GIT_COMMITTER_DATE=f'2026-01-0{number}T12:00:00Z')
            self.git('commit', '-q', '-m', f'change {number}', env=environment)

    def git(self, *args, env=None):
        return subprocess.run(['git', *args], cwd=self.repo, env=env, check=True,
                              text=True, capture_output=True).stdout

    def write(self, name, text):
        path = self.repo / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def test_real_analysis_counts_changes_and_coupling(self):
        java, version = MAAT.java_runtime()
        jar = MAAT.install_jar(self.config)
        output = MAAT.report(self.repo, '2026-01-01', 100, self.config, java, version, jar)
        with (output / 'hotspots.csv').open() as stream:
            rows = {row['entity']: row for row in csv.DictReader(stream)}
        self.assertEqual(set(rows), {'src/a.rs', 'src/b.rs'})
        self.assertEqual(rows['src/a.rs']['revisions'], '3')
        self.assertEqual(rows['src/a.rs']['added'], '3')
        self.assertEqual(rows['src/a.rs']['deleted'], '0')
        with (output / 'coupling.csv').open() as stream:
            pairs = list(csv.DictReader(stream))
        self.assertEqual(len(pairs), 1)
        self.assertEqual({pairs[0]['entity'], pairs[0]['coupled']}, {'src/a.rs', 'src/b.rs'})
        self.assertEqual(pairs[0]['degree'], '80')
        self.assertNotIn('generated.js', (output / 'history.log').read_text())
        self.assertNotIn('Fixture', (output / 'history.log').read_text())

    def test_history_limit_and_empty_window_are_explicit(self):
        _, _, count, capped = MAAT.history(self.repo, '2026-01-01', 1, self.config['paths'])
        self.assertEqual((count, capped), (1, True))
        _, log, count, capped = MAAT.history(self.repo, '2027-01-01', 100, self.config['paths'])
        self.assertEqual((log, count, capped), ('', 0, False))

    def test_shallow_history_is_rejected(self):
        clone = self.repo.parent / 'shallow'
        self.git('clone', '-q', '--depth=1', self.repo.as_uri(), str(clone))
        with self.assertRaisesRegex(RuntimeError, 'complete history'):
            MAAT.history(clone, '2026-01-01', 100, self.config['paths'])

    def test_invalid_analyzer_output_is_not_accepted_as_a_report(self):
        with patch.object(MAAT, 'run', return_value='analysis failed silently\n'):
            with self.assertRaisesRegex(RuntimeError, 'Unexpected Code Maat revisions output'):
                MAAT.analyze('java', Path('unused.jar'), Path('unused.log'), self.repo, self.config)

    def test_corrupt_cached_download_is_rejected(self):
        path = self.repo / 'bad.jar'
        path.write_bytes(b'not the release asset')
        with self.assertRaisesRegex(RuntimeError, 'checksum mismatch'):
            MAAT.verify_jar(path, self.config['sha256'])


if __name__ == '__main__':
    unittest.main()
