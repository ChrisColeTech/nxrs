"""bug_report.py on a made-up game folder: the allowed files only, redacted, and nothing from the data folder.
    py -m unittest scripts/test_bug_report.py"""
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import bug_report  # noqa: E402


class BugReport(unittest.TestCase):
    def test_folder_to_zip(self):
        with tempfile.TemporaryDirectory() as d:
            g = Path(d)
            (g / 'game.exe').write_bytes(b'MZ')
            (g / 'game_code.dll').write_bytes(b'MZ')
            home = str(Path.home())
            (g / 'game.log').write_text(f'started from {home}\games\nheader_key = 0123456789abcdef0123456789abcdef\nok\n')
            (g / 'game.ini').write_text('; comment\nlog=1\n')
            (g / 'game_data' / 'exefs').mkdir(parents=True)
            (g / 'game_data' / 'exefs' / 'main').write_bytes(b'NSO0' * 100)
            (g / 'metrics-1.csv').write_text('a,b\n1,2\n')
            out = g / 'r.zip'
            self.assertEqual(bug_report.main([str(g / 'game.exe'), '--out', str(out)]), 0)
            with zipfile.ZipFile(out) as z:
                names, log, rep = sorted(z.namelist()), z.read("game.log").decode(), z.read("report.md").decode()
            self.assertEqual(names, ['game.ini', 'game.log', 'metrics.csv', 'report.md'])
            self.assertNotIn(home, log)
            self.assertIn('~', log)
            self.assertNotIn('0123456789abcdef0123', log)
            self.assertIn('log=1', rep)
            self.assertEqual(bug_report.main([str(g / 'game_data')]), 2)


if __name__ == '__main__':
    unittest.main()
