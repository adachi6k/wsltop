# Proposed title

wsltop: Add version 0.5.0

# Submission prerequisite

This is a proposal, not a submitted PR. Obtain agreement on the package request
and Main eligibility first, then replace the issue placeholder below. The
project currently falls below Main's stated popularity benchmark.

# Proposed body

Relates to #<approved-package-request>

Add wsltop 0.5.0, an MIT-licensed portable CLI for monitoring Windows, WSL2,
Docker and WSL Containers. Windows 11 and a usable WSL2 distribution are
required at runtime; Docker and WSLC are optional.

The manifest installs the official Windows x64 release ZIP. `extract_dir`
selects `wsltop-v0.5.0-x86_64-pc-windows-msvc`, and `bin` exposes `wsltop.exe`
as `wsltop`. GitHub checkver and autoupdate cover the versioned ZIP URL,
SHA256 sidecar and nested directory.

Validation on Windows PowerShell 5.1:

- Official Scoop bucket schema/style tests: 7 passed, 0 failed, 0 skipped.
- Scoop formatjson, checkver and checkurls passed.
- Forced autoupdate to 0.5.0 retrieved the expected SHA256 and directory.
- Downloaded archive SHA256 and nested executable were independently checked.
- Local manifest install, `wsltop` shim `--version` / `--help`, and uninstall
  passed in a temporary Scoop root. The installed app and shim were removed.

Only `bucket/wsltop.json` is included in the upstream change.

- [x] Use conventional PR title: `wsltop: Add version 0.5.0`
- [x] I have read the [Contributing Guide](https://github.com/ScoopInstaller/.github/blob/main/.github/CONTRIBUTING.md).
