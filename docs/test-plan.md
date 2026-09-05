# Validation and Test Plan

This document separates automated CI checks from real Windows/WSL host validation. CI runs portable checks on Ubuntu, a Windows GNU cross-target check, and native Windows tests, an MSVC release build, and help/version smoke checks. Collectors that require WSL2, Windows interoperability, WSLC, Docker, or multiple distributions must be exercised on representative hosts before release.

Recorded runs: [2026-09-05 Windows-native archive and TUI smoke tests](validation/2026-09-05-windows-native.md).

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
rustup target add x86_64-pc-windows-gnu
cargo check --locked --all-targets --target x86_64-pc-windows-gnu
```

Unit tests cover CPU delta normalization, resource classification, attribution remainder/clamping, Windows application/WebView2 grouping, WSLC ambiguity, Docker/WSLC process parsing and nesting, host normalization, malformed-output degradation, and grouped flat filtering. Help and version smoke tests must exit before any WSL/Windows runtime collection.

Streaming regressions also cover case-insensitive primary exclusion and running
membership, runtime discovery from an empty additional-collector set, loading
through baseline-only updates, last-good/baseline retention, the two-second
additional-collector cadence floor, and shutdown polling latency.

## Windows-native and WSL-native smoke procedure

Use a Windows 11 + WSL2 host with a primary distro and a second disposable test
distro. Record the commit/tag, Windows and WSL versions, distro names, host/guest
CPU counts, terminal, and available Docker/WSLC versions. Run the same scenarios
with both executables; a cross-target compile is not a runtime test.

### Windows PowerShell

Build from a checkout with the MSVC Rust toolchain and Visual Studio C++ build
tools, or extract the Windows release ZIP using the README checksum procedure.
For a source build:

```powershell
cargo build --release --locked --target x86_64-pc-windows-msvc
$exe = '.\target\x86_64-pc-windows-msvc\release\wsltop.exe'
& $exe --help
& $exe --version
wsl.exe --list --verbose
& $exe --once --cpu-scale host
& $exe --distro Ubuntu-24.04 --once --json | ConvertFrom-Json
& $exe --distro Ubuntu-24.04 --tree --json | ConvertFrom-Json
& $exe --distro Ubuntu-24.04 --wsl-only --no-docker
& $exe --distro Ubuntu-24.04 --interactive --interval-ms 1000
```

Replace `Ubuntu-24.04` with an installed distro. Check default selection without
`--distro`, explicit selection (including differing letter case), and a nonexistent
name. The explicit primary must appear only once, with the JSON source omitted;
additional processes carry their distro name. A nonexistent required primary
must fail a one-shot sample; interactive sampling reports the error and remains
responsive. Where default discovery is unavailable, the running-distro fallback
must select a primary or report that no distro is available. This fallback is
also covered by injected unit tests; do not change a working host configuration
merely to force it.

### WSL shell

From a checkout inside the primary distro:

```console
cargo build --release --locked
./target/release/wsltop --once --cpu-scale host
./target/release/wsltop --json
./target/release/wsltop --tree --json
./target/release/wsltop --wsl-only --no-docker
./target/release/wsltop --interactive --interval-ms 1000
./target/release/wsltop --distro Ubuntu-24.04
```

The final command must reject the Windows-only option. The local distro remains
the primary with no source label. In `--wsl-only`, verify the WSL-visible CPU
denominator and normalization warning; on Windows the denominator is Windows-visible.

### Interactive lifecycle on both platforms

1. Start with the test distro stopped. Confirm the loading frame appears before
   collection completes, primary rows arrive independently, and flat/tree views,
   scrolling, `i`, `h`, and `0` work.
2. In another terminal start the test distro and keep a shell/workload running:
   `wsl.exe -d <test-distro>`. Verify it is discovered without restarting wsltop.
   Allow two additional-collector passes for its baseline and CPU delta; optional
   discovery must not block primary refreshes. Test startup with that distro
   already running too: baseline-only success must not prematurely end loading.
3. Exit all work in the disposable distro, then use `wsl.exe --terminate <test-distro>`
   only after saving its work. Confirm its rows disappear after discovery confirms
   the stop, the primary remains visible, and restarting it establishes a new baseline.
4. Exercise an optional collector failure and recovery. Last-good rows remain
   visible with an error until a successful update; failure is distinct from a
   confirmed stopped distro. Deterministic transient-discovery and snapshot failures
   are unit-tested when the real host cannot reproduce them safely.
5. Quit with `q` and with `Esc`, including during a long configured interval.
   Verify prompt exit, restored echo/cursor/alternate screen, and a usable shell.
   Check `Ctrl-C` where supported. Cadence waits poll every 50 ms; this does not
   guarantee cancellation of an in-flight `wsl.exe` command.
6. Run `--interactive --json` and verify explicit rejection without terminal damage.

Record each scenario as passed, failed, or not run, with evidence. In particular,
Windows-native TUI runtime validation is a release acceptance item; adding this
procedure or passing CI does not mark it complete.

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

Before tagging a release:

1. All automated commands pass from a clean checkout using `Cargo.lock`.
2. The feature matrix is exercised on at least one current Windows 11 + WSL2 host.
3. CPU normalization is checked against one-CPU and four-CPU workloads.
4. At least one optional-collector failure path is verified for WSLC and Docker.
5. Validate both Linux x86_64 tar.gz and Windows x86_64 MSVC ZIP packaging in a dry run: each archive contains its executable, README, and LICENSE in a versioned directory; each SHA-256 file matches the archive; extracted executables pass help/version. Do not push a test `v*` tag merely to validate packaging, because it publishes a release.
6. README installation and quick-start commands work as written.
7. Record native Windows and WSL interactive results, including distro discovery,
   baseline loading, failure recovery, and terminal restoration.

The tagged release workflow checks the tag against `Cargo.toml`, builds on each
native runner, and publishes both platforms' archives/checksums only after both
packaging jobs succeed. It does not publish to crates.io. Packaging verification
and real-host results should be attached to the release PR; skipped checks remain
explicitly pending.

Changes to the release workflow run the same packaging jobs in pull requests.
The workflow also supports a manual `workflow_dispatch` dry run. Both use the
package version for archive names, verify transferred checksums and extracted
executables, and upload downloadable Actions artifacts without publishing a
GitHub release. Only a `v*` tag push reaches the publication step. Download those
artifacts to perform the Windows/WSL runtime procedure on the exact packaged build.
