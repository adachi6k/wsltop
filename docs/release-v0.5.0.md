# v0.5.0 — Compact resource summary and TUI improvements

wsltop v0.5.0 makes host and guest resource consumption easier to read while
preserving top-like simplicity. It is a visual improvement, not an htop clone.
The CLI and TUI run natively on Windows and WSL.

## Highlights

- Compact two-line CPU/RAM summary is now the default TUI header.
- Host-wide CPU utilization and physical RAM in use/total remain visible above
  the resource list, independently of row limits, filters and display scale.
- Windows, WSL, WSLC and Docker observations use consistent environment colors.
- Compact CPU/RAM history graphs show recent changes on a shared time axis.
- Aligned columns, subtle separators and a single status/key footer improve
  readability without adding per-core panels or more resource columns.
- The classic one-line header remains available with `--header classic`.

Use `--color auto|always|never` to control colors. Auto respects nonempty
`NO_COLOR` and `TERM=dumb`; `always` explicitly enables color. On terminals below
80 columns the summary shows totals only. Medium/wide terminals show 15/23
history columns, spanning 45/69 seconds at the default three-second interval.
Missing initial data is blank in history; waiting slots hold the previous value,
and failed readings show `!` until collection recovers. Press `?` for details.

Environment observations are not an additive partition of host usage. WSL can
include Docker workloads; process working sets, RSS and container memory statistics
have different definitions. Host physical RAM is collected separately.

## Reliability and compatibility

This release aligns CPU/RAM history timestamps, fixes narrow-terminal help
scrolling, restores classic header fields and polishes tree rendering. Windows
command execution includes Job Object and timeout-race fixes. Windows refresh
periods account for query duration without overlapping collection.
Final acceptance testing also fixed restoration of the caller's exact Windows
console input/output modes after leaving the TUI.

JSON fields/schema and the host-wide CPU accounting model are unchanged.
Existing sorting, container grouping and key bindings are retained. No process
actions, MCP integration, new collectors or machine-readable fields are added.
Release preparation changes versioning, documentation, validation and packaging,
plus the console-mode restoration fix found by its real-host acceptance test.

## Distribution

The release retains these archive names, each with a `.sha256` sidecar:

- `wsltop-v0.5.0-x86_64-pc-windows-msvc.zip`
- `wsltop-v0.5.0-x86_64-unknown-linux-gnu.tar.gz`

Each archive contains its executable, README and LICENSE under the versioned
directory. Windows requires Windows 11 and a usable primary WSL2 distribution;
`--distro NAME` selects that primary on Windows only. Docker and WSLC are optional.

This document is prepared ahead of publication. The release-preparation PR records
candidate validation; tagging, GitHub Release publication, crates.io publication
and public download/install verification are separate, approval-gated steps.
