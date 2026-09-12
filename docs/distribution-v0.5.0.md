# v0.5.0 distribution validation

Validated on 2026-09-12. Existing GitHub release names and published v0.5.0
assets are unchanged. This change adds metadata and a Scoop proposal, without
publishing another crate version or replacing released artifacts.

## cargo-binstall

`Cargo.toml` now specifies separate target overrides for:

| Target | Archive | Format |
| --- | --- | --- |
| `x86_64-unknown-linux-gnu` | `wsltop-v{version}-x86_64-unknown-linux-gnu.tar.gz` | `tgz` |
| `x86_64-pc-windows-msvc` | `wsltop-v{version}-x86_64-pc-windows-msvc.zip` | `zip` |

Both use GitHub Releases under `v{version}`. The binary path template is
`wsltop-v{version}-{target}/{bin}{binary-ext}`, matching the actual archives.
Configuration follows the [cargo-binstall maintainer documentation](https://github.com/cargo-bins/cargo-binstall/blob/main/SUPPORT.md).

Published crate metadata is immutable. Adding this configuration to the
repository does not alter the already-published 0.5.0 crate; the metadata will
be distributed by a future crate publication. The public 0.5.0 crate already
works with binstall's GitHub asset discovery, as verified separately below.

Using official cargo-binstall **1.23.0** binaries, all four installs passed:

| Host | Metadata source | Result |
| --- | --- | --- |
| WSL/Linux x64 | Published crates.io 0.5.0, automatic GitHub asset discovery | Passed |
| WSL/Linux x64 | Updated local `Cargo.toml` via `--manifest-path` | Passed |
| Native Windows x64 | Published crates.io 0.5.0, automatic GitHub asset discovery | Passed |
| Native Windows x64 | Updated local `Cargo.toml` via `--manifest-path` | Passed |

Every invocation selected only `--strategies crate-meta-data`, making source
builds and third-party quick-install downloads unavailable. Installed binary
hashes match the official GitHub archive contents. All report `wsltop 0.5.0`;
Windows installs also passed `--help`. The actual `cargo binstall wsltop`
subcommand was exercised on Linux with the same strategy restriction.

Example commands for repeating the two Linux checks in isolated directories:

```sh
cargo binstall wsltop --version 0.5.0 --strategies crate-meta-data \
  --no-confirm --no-track --disable-telemetry --install-path /tmp/wsltop-binstall-public
cargo binstall wsltop --version 0.5.0 --manifest-path . \
  --strategies crate-meta-data --no-confirm --no-track --disable-telemetry \
  --install-path /tmp/wsltop-binstall-metadata
```

On Windows, use a Windows-local directory such as
`--install-path "$env:TEMP\wsltop-binstall-public"`. Installation onto a WSL UNC
share failed with Windows error 80; repeating the check on a local Windows
filesystem succeeded for both metadata sources. No installed user binary was
replaced.

## Scoop

The proposed [manifest](../packaging/scoop/wsltop.json) uses the official ZIP:

`https://github.com/adachi6k/wsltop/releases/download/v0.5.0/wsltop-v0.5.0-x86_64-pc-windows-msvc.zip`

SHA256:
`ca5164d22c0b0da3e52f5519dc5f2e1d6777e0cc38f0cfb5dd42d1c36fa1af17`

The ZIP's `wsltop-v0.5.0-x86_64-pc-windows-msvc/wsltop.exe` was independently
verified. `extract_dir` strips the versioned directory before the `bin` shim is
created. MIT license, four-space formatting, CRLF, GitHub checkver, versioned
autoupdate URL/directory and release-sidecar hash extraction are configured.

Validation used Windows PowerShell 5.1, Pester 6.2.0 and the official Scoop
`test/Import-Bucket-Tests.ps1` with a bucket containing only the candidate:

- Scoop core: `b588a06e41d920d2123ec70aee682bae14935939`.
- Main reference checkout: `cac179f93266355bf8be04d35ead70cad9c50985`.
- Official schema/style tests: **7 passed, 0 failed, 0 skipped**.
- `formatjson.ps1`: applied; the final formatted manifest is checked in.
- `checkver.ps1`: detected **0.5.0**.
- `checkurls.ps1`: **1 URL, 1 okay, 0 failed**.
- Forced `checkver.ps1 -Version 0.5.0 -ForceUpdate`: retrieved the expected
  SHA256 from the release sidecar and retained the correct extraction directory.
- Manifest install in a temporary Windows-local Scoop root: passed download,
  hash check, extraction, linking and `wsltop` shim creation.
- The shim's `--version` and `--help`: passed.
- Uninstall: removed the application directory and shim. The temporary Scoop
  shim entry added to the user's PATH by initial Scoop setup was removed.

The test runner and app installation used Windows-local temporary directories
because Pester's repository discovery did not accept the WSL UNC working path.
Initial setup also reported a missing default bucket; the explicit local
manifest installation and uninstall still completed successfully.

These are technical validation results, not confirmation of Main eligibility.
See the [submission status and proposal](../packaging/scoop/README.md) for the
Main popularity benchmark and required prior maintainer discussion. No upstream
Scoop issue or PR has been submitted.
