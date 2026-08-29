# Validation and Test Plan

This document separates portable CI checks from real Windows/WSL host validation. CI validates deterministic code paths on an Ubuntu runner; collectors that require Windows interoperability, WSLC, Docker, or multiple WSL distributions must be exercised on representative hosts before release.

## Automated checks

Run from a clean checkout with the tracked lockfile:

```console
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --release --locked
cargo package --locked
cargo run --locked -- --help
cargo run --locked -- --version
```

Unit tests cover CPU delta normalization, resource classification, attribution remainder/clamping, Windows application/WebView2 grouping, WSLC ambiguity, Docker/WSLC process parsing and nesting, host normalization, malformed-output degradation, and grouped flat filtering. Help and version smoke tests must exit before any WSL/Windows runtime collection.

## Real-host feature matrix

| Case | Setup / command | Expected result |
| --- | --- | --- |
| Windows native processes | `wsltop --once` with interop enabled | Windows rows appear and use host-wide CPU normalization |
| Windows application grouping | Run Teams, Chrome, ChatGPT, and SearchHost/WebView2 workloads | Flat/TUI ranks application totals once; tree nests contributing PIDs; application TIME+ sums live members; SearchHost-owned and ambiguous WebView2 helpers are not assigned to Teams |
| Current WSL distro | Run a known workload in the invoking distro | Process appears with no remote-distro source label |
| Second WSL distro | Start another distro, run a workload, then `wsltop --once` | Workload appears labelled with its distro; PID collisions do not merge |
| WSLC present | Run containers in the default WSLC session | Container rows appear as `WSLC container` |
| WSLC process attribution | Run identifiable processes in WSLC using the default flat view and `--tree` | Processes and available TIME+ appear beneath their WSLC container; the collector's temporary `ps` is absent; an unsupported `time` column falls back without losing rows |
| WSLC absent | Run without `wslc.exe` installed | WSLC rows are silently omitted; Windows, WSL, and Docker collection continue |
| Multiple WSLC hosts | Make more than one `vmmemwslc-*` host visible | Tree reports session mapping unresolved and does not guess; flat containers remain |
| Docker present | Run a busy container | Docker container appears and is host-normalized |
| Docker CLI absent | Remove Docker CLI from `PATH` | Docker rows are silently omitted; non-Docker monitoring continues |
| Docker daemon stopped | Stop the daemon or make it unreachable with a recognized connection error | Docker rows are silently omitted; other collectors continue |
| Unexpected optional collector error | Cause malformed output or an unrecognized command failure | Monitoring continues and the common warning path reports the failure in CLI/TUI |
| Docker process attribution | Run identifiable processes in a busy Docker Desktop container, then use the default flat view and `--tree` | Container nests Docker-native processes with available TIME+ under an independent Docker group and is not attached to the current WSL VM |
| Grouped flat limits | Use `--container-process-limit 2 --limit 5` | Five top-level resources are ranked by their own CPU; each selected container shows at most two processes plus omitted/residual rows |
| Tree output | `wsltop --tree` | WSL/WSLC hosts are parents with children and non-negative unattributed CPU |
| Flat JSON | `wsltop --json` | Top-level value remains the compatible PID-level resource array with `kind` fields and optional additive `cpu_time_seconds` |
| Tree JSON | `wsltop --tree --json` | Structured object includes host CPU count, additive Windows application groups, attribution groups, residuals, and unresolved resources |
| Interactive TUI | `wsltop --interactive` | An immediate loading frame is followed by current-WSL data after the short warmup; slow optional collectors do not block it and keyboard navigation remains responsive |
| Partial collector failure | Allow a collector to succeed, then fail it during TUI operation | Its last successful rows remain visible and the footer reports the error while other collectors continue updating |
| Hidden container detail | Start with `--hide-container-processes`, then press `t` | Flat output omits process rows; tree view requests and displays process detail on the next slow refresh |
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

- WSLC not installed: silently continue with Windows and WSL data.
- Docker CLI missing or a recognized daemon-unavailable error: silently continue without Docker rows.
- Unexpected WSLC/Docker command, parse, or attribution error: continue and surface a common warning in CLI/TUI.
- Additional distro exits or is unavailable mid-sample: warn and continue with remaining sources.
- Multiple WSLC hosts: mark mapping unresolved and preserve ungrouped resources.
- Collector warning in TUI: surface it in the status line without terminating refreshes.
- Terminal exit, collector error, panic/unwind path, and `Ctrl-C` where supported: raw mode, alternate screen, and cursor state return to normal.

## Release acceptance

Before tagging v0.2.0:

1. All automated commands pass from a clean checkout using `Cargo.lock`.
2. The feature matrix is exercised on at least one current Windows 11 + WSL2 host.
3. CPU normalization is checked against one-CPU and four-CPU workloads.
4. At least one optional-collector failure path is verified for WSLC and Docker.
5. The release workflow produces an executable Linux x86_64 archive from a test tag or dry run.
6. README installation and quick-start commands work as written.
