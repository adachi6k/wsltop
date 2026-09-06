import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import zipfile

from winget import manifests, verify_archive, version_from_tag

spec = importlib.util.spec_from_file_location('submit_winget', Path(__file__).with_name('submit-winget.py'))
submit_winget = importlib.util.module_from_spec(spec)
spec.loader.exec_module(submit_winget)


class WingetTests(unittest.TestCase):
    def test_fork_waits_for_repository_and_git_data(self):
        fork = {'fork': True, 'parent': {'full_name': 'microsoft/winget-pkgs'}, 'default_branch': 'master'}
        replies = [None, fork, RuntimeError('HTTP 409'), fork, None, fork, {'object': {'sha': 'ready'}}]
        with patch.object(submit_winget, 'api', side_effect=replies), patch.object(submit_winget.time, 'sleep') as sleep:
            self.assertEqual(submit_winget.wait_for_fork('repos/tester/winget-pkgs'), fork)
            self.assertEqual([c.args[0] for c in sleep.call_args_list], [1, 2, 4])

    def test_fork_wait_is_bounded_and_does_not_hide_permission_errors(self):
        with patch.object(submit_winget, 'api', return_value=None) as api, patch.object(submit_winget.time, 'sleep'):
            with self.assertRaisesRegex(RuntimeError, 'provisioning'):
                submit_winget.wait_for_fork('repos/tester/winget-pkgs')
            self.assertEqual(api.call_count, 6)
        with patch.object(submit_winget, 'api', side_effect=RuntimeError('HTTP 403')):
            with self.assertRaisesRegex(RuntimeError, 'HTTP 403'):
                submit_winget.wait_for_fork('repos/tester/winget-pkgs')

    def test_existing_upstream_version_does_not_mutate(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for name, content in manifests('v0.4.0', '0' * 64, 'wsltop.exe').items():
                (directory / name).write_text(content)
            with patch.object(submit_winget, 'api', return_value=[{'name': 'manifest'}]) as api:
                submit_winget.submit(directory, 'v0.4.0')
                api.assert_called_once_with(
                    'repos/microsoft/winget-pkgs/contents/manifests/a/Adachi6k/wsltop/0.4.0', missing_ok=True)

    def test_existing_branch_with_unrelated_changes_is_not_submitted(self):
        import base64
        contents = manifests('v0.4.0', '0' * 64, 'wsltop.exe')
        def fake_api(path, data=None, missing_ok=False):
            self.assertIsNone(data, 'Existing-branch validation must not mutate anything')
            if path.startswith('repos/microsoft/winget-pkgs/contents/'):
                return None
            if path.startswith('search/issues?'):
                return {'items': []}
            if path == 'user':
                return {'login': 'tester'}
            if path == 'repos/tester/winget-pkgs':
                return {'fork': True, 'parent': {'full_name': 'microsoft/winget-pkgs'}}
            if '/git/ref/' in path:
                return {'object': {'sha': 'existing'}}
            if '/contents/' in path:
                name = path.split('/')[-1].split('?')[0]
                return {'content': base64.b64encode(contents[name].encode()).decode()}
            if '/compare/' in path:
                return {'files': [{'filename': 'unrelated.yaml', 'status': 'added'}]}
            self.fail(path)
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            for name, content in contents.items():
                (directory / name).write_text(content)
            with patch.object(submit_winget, 'api', side_effect=fake_api):
                with self.assertRaisesRegex(ValueError, 'only this version'):
                    submit_winget.submit(directory, 'v0.4.0')

    def test_stable_tags_only(self):
        for invalid in ["v0.4.0-rc1", "0.4.0", "v01.2.3", "v1.2.3/../bad", "v1.2.3\n"]:
            with self.assertRaises(ValueError):
                version_from_tag(invalid)
        self.assertEqual(version_from_tag("v12.34.56"), "12.34.56")

    def test_versioned_layout_and_hash_are_derived_together(self):
        with tempfile.TemporaryDirectory() as temp:
            archive = Path(temp) / "wsltop-v0.5.0-x86_64-pc-windows-msvc.zip"
            directory = archive.stem
            with zipfile.ZipFile(archive, "w") as zipped:
                for name, content in [("wsltop.exe", b"MZfixture"), ("README.md", b"readme"), ("LICENSE", b"MIT")]:
                    zipped.writestr(f"{directory}/{name}", content)
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            sidecar = archive.with_suffix('.zip.sha256')
            sidecar.write_text(f"{digest}  {archive.name}\n")
            actual, nested = verify_archive(archive, sidecar, "v0.5.0")
            files = manifests("v0.5.0", actual, nested)
            installer = files['Adachi6k.wsltop.installer.yaml']
            self.assertIn(f"InstallerSha256: {digest.upper()}", installer)
            self.assertIn(f"RelativeFilePath: {directory}\\wsltop.exe", installer)
            self.assertIn('/v0.5.0/wsltop-v0.5.0-', installer)
            sidecar.write_text(f"{'0' * 64}  {archive.name}\n")
            with self.assertRaisesRegex(ValueError, 'checksum'):
                verify_archive(archive, sidecar, "v0.5.0")
            with zipfile.ZipFile(archive, "a") as zipped:
                zipped.writestr('../unexpected.exe', b'MZ')
            digest = hashlib.sha256(archive.read_bytes()).hexdigest()
            sidecar.write_text(f"{digest}  {archive.name}\n")
            with self.assertRaisesRegex(ValueError, 'layout'):
                verify_archive(archive, sidecar, "v0.5.0")


if __name__ == '__main__':
    unittest.main()
