"""验证发行附件的失败边界；不访问 GitHub 或用户文件。"""
import hashlib
import importlib.util
import io
from pathlib import Path
import unittest
import zipfile

SPEC = importlib.util.spec_from_file_location('publish_release', Path(__file__).parents[1] / 'scripts/publish-release.py')
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ReleaseArtifacts(unittest.TestCase):
    def archive(self, files):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, 'w') as archive:
            for name, content in files:
                archive.writestr(name, content)
        data = stream.getvalue()
        return data, 'sha256:' + hashlib.sha256(data).hexdigest()

    def test_valid_installers_and_checksums(self):
        name = 'TermRill_0.2.0_x64.dmg'
        content = b'fictional installer'
        checksum = hashlib.sha256(content).hexdigest() + '  ' + name + '\n'
        data, digest = self.archive([(name, content), (name + '.sha256', checksum)])
        self.assertEqual(MODULE.verified_archive(data, digest, MODULE.installer_names('0.2.0'))[name], content)

    def test_corrupted_archive_rejected(self):
        data, digest = self.archive([])
        with self.assertRaises(ValueError):
            MODULE.verified_archive(data + b'changed', digest, set())

    def test_unexpected_paths_and_old_versions_rejected(self):
        for name in ['../escape', '/absolute', 'TermRill_0.1.0_x64.dmg']:
            data, digest = self.archive([(name, b'x')])
            with self.assertRaises(ValueError):
                MODULE.verified_archive(data, digest, MODULE.installer_names('0.2.0'))

    def test_missing_or_invalid_installer_checksum_rejected(self):
        name = 'TermRill_0.2.0_x64.dmg'
        for files in [[(name, b'x')], [(name, b'x'), (name + '.sha256', b'bad')]]:
            data, digest = self.archive(files)
            with self.assertRaises(ValueError):
                MODULE.verified_archive(data, digest, MODULE.installer_names('0.2.0'))
