import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parent))
import library_play  # noqa: E402,F401
import play_scenario as ps  # noqa: E402


class SaveSafety(unittest.TestCase):
    def test_unconfirmed_process_cleanup_retains_backup_without_restoring_live_save(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            save = root / 'data/save'
            run = root / 'run'
            save.mkdir(parents=True)
            run.mkdir()
            (save / 'slot').write_bytes(b'original')
            options = SimpleNamespace(data=str(save.parent), env=[], option=[], seconds=1,
                                      capture_interval=0, diagnostics='baseline')
            def failed_run(*args, **kwargs):
                (save / 'slot').write_bytes(b'possibly still writing')
                raise ps.run_game.harness_job.CleanupError('injected drain failure')
            with mock.patch.object(ps.run_game, 'run', side_effect=failed_run), \
                    mock.patch.object(ps.run_game.gpu_stop, 'paths', return_value=(root / 'stop.json',)), \
                    mock.patch.object(ps, 'PM', []):
                with self.assertRaisesRegex(ps.run_game.harness_job.CleanupError, 'drain failure'):
                    ps.scenario(options, dict(save=True, seconds=1, steps=[]), run, root / 'fake.exe')
            self.assertEqual((run / 'save-before/slot').read_bytes(), b'original')
            self.assertEqual((save / 'slot').read_bytes(), b'possibly still writing')
            self.assertFalse((run / 'save-restoration.json').exists())

    def test_changed_save_is_preserved_and_original_restored(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            save = root / 'data/save'
            run = root / 'run'
            save.mkdir(parents=True)
            run.mkdir()
            (save / 'slot').write_bytes(b'original')

            tx = ps.SaveTransaction(save, run).begin()
            (save / 'slot').write_bytes(b'changed')
            (save / 'new').write_bytes(b'created')
            changes = tx.finish()

            self.assertEqual((save / 'slot').read_bytes(), b'original')
            self.assertFalse((save / 'new').exists())
            self.assertEqual((run / 'save-after/slot').read_bytes(), b'changed')
            self.assertEqual(changes['added'], ['new'])
            self.assertEqual(changes['modified'], ['slot'])
            self.assertTrue(json.loads((run / 'save-changes.json').read_text())['changed'])

    def test_new_save_is_preserved_then_removed(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            save = root / 'data/save'
            run = root / 'run'
            run.mkdir()
            tx = ps.SaveTransaction(save, run).begin()
            save.mkdir(parents=True)
            (save / 'slot').write_bytes(b'new game')
            tx.finish()
            self.assertFalse(save.exists())
            self.assertEqual((run / 'save-after/slot').read_bytes(), b'new game')

    def test_evidence_failure_still_restores_original(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            save = root / 'data/save'
            run = root / 'run'
            save.mkdir(parents=True)
            run.mkdir()
            (save / 'slot').write_bytes(b'original')
            tx = ps.SaveTransaction(save, run).begin()
            (save / 'slot').write_bytes(b'changed')
            real_copytree = ps.shutil.copytree

            def fail_after(src, dst, *args, **kwargs):
                if Path(dst) == run / 'save-after':
                    raise OSError('evidence disk full')
                return real_copytree(src, dst, *args, **kwargs)

            with mock.patch.object(ps.shutil, 'copytree', side_effect=fail_after):
                with self.assertRaisesRegex(OSError, 'disk full'):
                    tx.finish()
            self.assertEqual((save / 'slot').read_bytes(), b'original')


class ScenarioMetadata(unittest.TestCase):
    def test_gameplay_scenarios_have_complete_acceptance_metadata(self):
        fields = {'target_fps', 'gameplay_window', 'max_stutters', 'allowed_gaps',
                  'memory_slope_threshold', 'max_red_errors', 'required_screenshot_names'}
        for name in ('wonder_gameplay', 'arceus_gameplay', 'violet_gameplay', 'jamboree'):
            scenario = ps.SCENARIOS[name]
            self.assertTrue(scenario['save'], name)
            self.assertLessEqual(fields, set(scenario['acceptance']), name)
            generated = set()
            for step in scenario['steps']:
                milestone = ps.capture_milestone(step[0], step[2])
                if milestone:
                    generated.add(milestone)
            required = scenario['acceptance']['required_screenshot_names']
            self.assertTrue(required, name)
            self.assertLessEqual(set(required), generated, name)

    def test_primary_library_scenarios_have_audited_acceptance(self):
        names = ('dkb', 'mkw', 'mkw-race', 'plza', 'zelda', 'mk8d', 'mk8d-lap', 'mk8d_gp', 'sonic_shadow',
                 'sonic_gens', 'sonic_gens2', 'shadow_skip', 'shadow_watch', 'smash_match', 'smash_long',
                 'agsob', 'vn')
        for name in names:
            scenario = ps.SCENARIOS[name]
            acceptance = scenario.get('acceptance')
            self.assertIsInstance(acceptance, dict, name)
            self.assertEqual(acceptance['max_red_errors'], 0, name)
            self.assertTrue(acceptance['required_screenshot_names'], name)
            milestones = {ps.capture_milestone(step[0], step[2]) for step in scenario['steps']
                          if ps.capture_milestone(step[0], step[2])}
            self.assertLessEqual(set(acceptance['required_screenshot_names']), milestones, name)

    def test_only_explicit_shots_and_replays_are_capture_milestones(self):
        beach_bounce = ps.SCENARIOS['vn']['steps']
        milestones = [ps.capture_milestone(step[0], step[2]) for step in beach_bounce]
        self.assertEqual(sum(name is not None for name in milestones), 12)
        self.assertIn('shot-boot', milestones)
        self.assertIn('shot-min 11', milestones)
        self.assertNotIn('a-a 0.0', milestones)

        self.assertEqual(ps.capture_milestone('replay', 'gameplay'), 'replay-gameplay')


if __name__ == '__main__':
    unittest.main()
