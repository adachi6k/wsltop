"""Print the tagged version's changelog entry for GitHub Releases."""
import pathlib
import re
import sys

tag = sys.argv[1]
if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
    sys.exit("expected a stable release tag such as v0.5.3")
changelog = pathlib.Path("CHANGELOG.md").read_text(encoding="utf-8")
entry = re.search(
    rf"(?ms)^## \[{re.escape(tag[1:])}\][^\n]*\n(.*?)(?=^## |\Z)", changelog
)
if entry is None or not entry[1].strip():
    sys.exit(f"missing changelog entry for {tag}")
notes = entry[1].strip()
notes = notes.replace(
    "](docs/", f"](https://github.com/adachi6k/wsltop/blob/{tag}/docs/"
)
print(notes)
