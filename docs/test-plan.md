# Validation and Test Plan

This document separates portable CI checks from real Windows/WSL host validation. CI validates deterministic code paths on an Ubuntu runner; collectors that require Windows interoperability, WSLC, Docker, or multiple WSL distributions must be exercised on representative hosts before release.

## Automated checks

Run from a clean checkout with the tracked lockfile:

```console
cargo fmt --all -- --check
cargo test --all-targets
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release --locked
cargo run --locked -- --help
cargo run --locked -- --version
```

Unit tests cover CPU delta normalization, resource classification, attribution remainder/clamping, WSLC ambiguity, Docker nesting, parsing, and flat filtering. Help and version smoke tests must exit before any WSL/Windows runtime collection.

## Real-host feature matrix

| Case | Setup / command | Expected result |
| --- | --- | --- |
| Windows native processes | `wsltop --once` with interop enabled | Windows rows appear and use host-wide CPU normalization |
| Current WSL distro | Run a known workload in the invoking distro | Process appears with no remote-distro source label |
| Second WSL distro | Start another distro, run a workload, then `wsltop --once` | Workload appears labelled with its distro; PID collisions do not merge |
| WSLC present | Run containers in the default WSLC session | Container rows appear as `WSLC container` |
| WSLC absent | Run without `wslc.exe` installed | Warning is emitted; Windows, WSL, and Docker collection continue |
| Multiple WSLC hosts | Make more than one `vmmemwslc-*` host visible | Tree reports session mapping unresolved and does not guess; flat containers remain |
| Docker present | Run a busy container | Docker container appears and is host-normalized |
| Docker CLI absent | Remove Docker CLI from `PATH` | Warning is emitted; non-Docker monitoring continues |
| Docker daemon stopped | Stop/unreachable daemon | Warning is emitted; no Docker rows; other collectors continue |
| Docker process attribution | Run identifiable processes in a busy container, `wsltop --tree` | Container nests matching processes; values are not double-counted as direct WSL children |
| Tree output | `wsltop --tree` | WSL/WSLC hosts are parents with children and non-negative unattributed CPU |
| Flat JSON | `wsltop --json` | Top-level value remains a resource array with `kind` fields |
| Tree JSON | `wsltop --tree --json` | Structured object includes host CPU count, groups, residuals, and unresolved resources |
| Interactive TUI | `wsltop --interactive` | Updates in place without spawning child `wsltop` processes; navigation/toggles work |
| Initial interactive options | Combine `--interactive` with interval, collector switches, limit, tree, infra, and host options | Initial state and all collection/filter choices are honored |
| Invalid interactive JSON | `wsltop --interactive --json` | Exits with an explicit incompatibility error and restores terminal state |
| Hide infrastructure | `wsltop --hide-infra` and toggle `i` in TUI | `plan9` infrastructure rows are hidden as selected |
| Show raw host | `wsltop --show-wsl-host` | Raw `vmmem*` host rows appear only in flat views |
| WSL-only | `wsltop --wsl-only` | Current distro and optional Docker data remain; Windows, additional distros, and WSLC are skipped; limitation warning appears |

## CPU validation

Use a workload that can pin a known number of logical CPUs and compare against Windows Task Manager over a stable interval.

On a 16-logical-CPU host:

| Workload | Expected host-wide value |
| --- | --- |
| One busy logical CPU | Approximately 6.25% |
| Four busy logical CPUs | Approximately 25% |

Validate Windows, current WSL, a second distro, WSLC, and Docker separately where possible. Use a sampling interval long enough that collector startup latency is small relative to the interval. Exact equality is not expected.

## Attribution validation

For each visible host group, verify:

```text
unattributed = max(host - known children, 0)
```

- host 10%, children 7% produces unattributed 3%
- host 10%, children 10% produces unattributed 0%
- host 10%, children 12% produces unattributed 0% and records 2% sampling skew
- child values are never proportionally scaled to fit the host
- parent memory is never subtracted from child memory

Confirm that parent and child CPU are not summed when interpreting total machine utilization.

## Degradation and recovery

Validate these failure paths deliberately:

- WSLC not installed: continue with Windows and WSL data.
- Docker CLI missing or daemon unavailable: continue without Docker rows.
- Additional distro exits or is unavailable mid-sample: warn and continue with remaining sources.
- Multiple WSLC hosts: mark mapping unresolved and preserve ungrouped resources.
- Collector warning in TUI: surface it in the status line without terminating refreshes.
- Terminal exit, collector error, panic/unwind path, and `Ctrl-C` where supported: raw mode, alternate screen, and cursor state return to normal.

## Release acceptance

Before tagging v0.1.0:

1. All automated commands pass from a clean checkout using `Cargo.lock`.
2. The feature matrix is exercised on at least one current Windows 11 + WSL2 host.
3. CPU normalization is checked against one-CPU and four-CPU workloads.
4. At least one optional-collector failure path is verified for WSLC and Docker.
5. The release workflow produces an executable Linux x86_64 archive from a test tag or dry run.
6. README installation and quick-start commands work as written.
