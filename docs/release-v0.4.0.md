# v0.4.0 — Native Windows support

## Highlights

- Native Windows CLI/TUI support alongside WSL-native execution.
- Windows x86_64 MSVC prebuilt release ZIP, with Linux x86_64 archives and SHA-256 checksums.
- CPU/memory/name sorting and ascending/descending direction, including immediate TUI controls: `c`, `m`, `n`, `r`.
- Shared query layer across CLI/TUI and JSON, preserving parent/container grouping and applying limits after sorting.

Use `wsltop.exe --interactive` on Windows or `wsltop --interactive` in WSL.
`--distro NAME` selects the primary WSL distribution for the Windows executable.
Windows 11 and a usable WSL2 distribution are required; Docker and WSLC are optional.

CPU descending remains the default. JSON field names and host-wide CPU values
remain compatible with v0.3.0; changing sort order does not change metric meaning.
Memory values differ across Windows/Linux/containers and should not be added
across attribution parents and children. Independently timed samples can produce
attribution skew; optional detail collectors may lag aggregate rows.

## Downloads

- `wsltop-v0.4.0-x86_64-pc-windows-msvc.zip`
- `wsltop-v0.4.0-x86_64-unknown-linux-gnu.tar.gz`

Each archive contains its executable, `README.md`, and `LICENSE` in a versioned
directory. Each `.sha256` sidecar uses `<sha256>  <filename>` format.

See [CHANGELOG](../CHANGELOG.md) for the full changes and
[release validation](validation/2026-09-06-v0.4.0.md) for verification scope.
