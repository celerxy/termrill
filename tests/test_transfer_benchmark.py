"""校验性能报告拒绝错误内容、禁止越界清理，命令执行超时不冒充成功。"""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import hashlib
import sys
from types import SimpleNamespace

spec = importlib.util.spec_from_file_location('benchmark', Path(__file__).parents[1] / 'scripts/benchmark-transfer.py')
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)


class BenchmarkTests(unittest.TestCase):
    def test_fixture_start_failure_closes_log(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            with patch.object(benchmark.subprocess, 'Popen', side_effect=OSError('fixture did not launch')):
                with self.assertRaises(OSError):
                    benchmark.start_fixture(root)
            # Windows 下未关闭的日志句柄会阻止删除。
            (root / 'fixture.log').unlink()

    def test_phase_export_is_a_numeric_whitelist(self):
        details = {'bytes': 4, 'connectMs': 1, 'transferMs': 2, 'maxProgressGapMs': 1,
                   'privatePath': 'private/example'}
        self.assertNotIn('privatePath', benchmark.checked_phases(details, 4))
        for value in ['private/example', float('nan'), -1, True]:
            with self.assertRaises(ValueError):
                benchmark.checked_phases({**details, 'transferMs': value}, 4)
    def test_low_disk_space_fails_before_source_or_key_changes(self):
        with tempfile.TemporaryDirectory() as folder:
            with patch.object(benchmark.shutil, 'disk_usage', return_value=SimpleNamespace(free=0)):
                with self.assertRaisesRegex(RuntimeError, 'free space'):
                    benchmark.prepare_source(Path(folder), {}, 8, 0)
            self.assertEqual(list(Path(folder).iterdir()), [])

    def test_existing_report_is_never_overwritten(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            args = SimpleNamespace(output=benchmark.PROJECT / 'README.md')
            before = args.output.read_bytes()
            with patch.object(benchmark, 'TESTS', root), patch.object(benchmark.shutil, 'which', return_value=sys.executable):
                with self.assertRaisesRegex(ValueError, 'new file'):
                    benchmark.benchmark(args)
            self.assertEqual(args.output.read_bytes(), before)
            self.assertEqual(list(root.iterdir()), [])

    def test_digest_and_length_are_both_required(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / 'sample.bin'
            path.write_bytes(b'1234')
            expected = hashlib.sha256(b'1234').hexdigest()
            self.assertEqual(benchmark.checked_download(path, 4, expected), expected)
            with self.assertRaises(ValueError):
                benchmark.checked_download(path, 3, expected)
            path.write_bytes(b'4321')
            with self.assertRaises(ValueError):
                benchmark.checked_download(path, 4, expected)

    def test_summary_keeps_concurrency_groups_separate(self):
        samples = [{'client': 'termrill', 'termrillConcurrency': 1, 'mibPerSecond': speed}
                   for speed in [1, 2, 30]]
        samples.append({'client': 'termrill', 'termrillConcurrency': 3, 'mibPerSecond': 10})
        result = benchmark.summarize(samples)
        self.assertEqual(result[0]['medianMiBPerSecond'], 2)
        self.assertEqual(result[0]['samples'], 3)
        self.assertEqual(result[1]['medianMiBPerSecond'], 10)

    def test_cleanup_rejects_repository_or_unrelated_paths(self):
        for path in [benchmark.PROJECT, benchmark.TESTS, benchmark.TESTS / '.runtime-benchmark-results']:
            with self.assertRaises(ValueError):
                benchmark.cleanup(path)

    def test_command_timeout_is_failure_and_quiet_completion_succeeds(self):
        with tempfile.TemporaryDirectory() as folder:
            result = benchmark.run_process([sys.executable, '-c', 'print("done")'], cwd=folder)
            self.assertEqual(result[0], 0)
            self.assertIn('done', result[1])
            with self.assertRaisesRegex(RuntimeError, 'timed out'):
                benchmark.run_process([sys.executable, '-c', 'import time; time.sleep(10)'], cwd=folder, timeout=0.2)

    def test_failure_logs_do_not_copy_keys_or_downloaded_content(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            runtime = root / 'runtime'
            runtime.mkdir()
            (runtime / 'client-key').write_text('private test key', encoding='utf-8')
            (runtime / 'sample.bin').write_bytes(b'private test content')
            (runtime / 'build.log').write_text('build failed', encoding='utf-8')
            with patch.object(benchmark, 'TESTS', root):
                benchmark.preserve_failure_logs(runtime, 'test')
            destination = root / '.runtime-benchmark-results'
            self.assertEqual([path.name for path in destination.iterdir()], ['failure-test.log'])
            self.assertNotIn('private test', (destination / 'failure-test.log').read_text(encoding='utf-8'))


if __name__ == '__main__':
    unittest.main()
