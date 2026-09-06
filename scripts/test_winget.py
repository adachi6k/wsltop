import hashlib
from pathlib import Path
import tempfile
import unittest
import zipfile

from winget import manifests, verify_archive, version_from_tag


class WingetTests(unittest.TestCase):
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
