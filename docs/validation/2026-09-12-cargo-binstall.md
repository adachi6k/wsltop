# v0.5.0 cargo-binstall validation

Validated on 2026-09-12. Existing GitHub release names and published v0.5.0
assets are unchanged. The cargo-binstall metadata was added without publishing
another crate version or replacing released artifacts.

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
