"""Exercise release-note selection using isolated changelog fixtures."""
import pathlib
import subprocess
import sys
import tempfile
import unittest


SCRIPT = pathlib.Path(__file__).with_name("release-notes.py").resolve()
CHANGELOG = """# Changelog
## [Unreleased]
Future work.
## [1.0.0] - 2026-09-26
Stable [policy](docs/compatibility.md).
## [1.0.0-rc.1] - 2026-09-26
Candidate [policy](docs/compatibility.md).
## [0.5.3] - 2026-09-16
Previous version.
"""


class ReleaseNotesTests(unittest.TestCase):
    def run_notes(self, tag, changelog=CHANGELOG):
        with tempfile.TemporaryDirectory() as directory:
            pathlib.Path(directory, "CHANGELOG.md").write_text(changelog, encoding="utf-8")
            return subprocess.run(
                [sys.executable, str(SCRIPT), tag], cwd=directory,
                capture_output=True, text=True, check=False,
            )

    def test_stable_and_rc_select_only_their_entry_and_pin_links(self):
        for tag, label in [("v1.0.0", "Stable"), ("v1.0.0-rc.1", "Candidate")]:
            with self.subTest(tag=tag):
                result = self.run_notes(tag)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, f"{label} [policy](https://github.com/adachi6k/wsltop/blob/{tag}/docs/compatibility.md).\n")

    def test_invalid_tags_are_rejected(self):
        for tag in ["1.0.0", "v01.0.0", "v1.0.0-rc.01", "v1.0.0-rc", "v1.0.0-beta.1", "v1.0.0/other"]:
            with self.subTest(tag=tag):
                result = self.run_notes(tag)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(result.stdout, "")

    def test_missing_or_empty_entry_is_rejected(self):
        for changelog in [CHANGELOG, "## [1.1.0]\n\n## [1.0.0]\nOld.\n"]:
            with self.subTest(changelog=changelog):
                result = self.run_notes("v1.1.0", changelog)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("missing changelog entry", result.stderr)


if __name__ == "__main__":
    unittest.main()
