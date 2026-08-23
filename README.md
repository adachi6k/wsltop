# wsltop

`wsltop` is a unified Windows, WSL, WSL Containers (WSLC), and Docker resource monitor for WSL2. It combines host and guest observations on one host-wide CPU scale and can explain the workload behind WSL virtual-machine CPU use.

> Windows Task Manager says `VmmemWSL` is busy. `wsltop` shows which Windows, WSL, WSLC, or Docker workload is responsible.

`wsltop` is currently a v0.1.0 release candidate. Its CLI and terminal UI are the supported interfaces; a graphical UI is outside the current scope.

## Why wsltop?

Task Manager may show only a VM aggregate such as `VmmemWSL 35%`. That identifies the host process, but not the guest workload. `wsltop --tree` makes the hierarchy visible:

```text
WSL VM
|- process    verilator
|- infra      plan9
`- container  Docker build
   `- process compiler
```

Parent and child CPU values are attribution views, not values to add together.

## Features

- Windows native process monitoring
- Current WSL distribution process monitoring through `/proc`
- Multiple running WSL distribution monitoring
- WSL Containers monitoring
- Docker container monitoring and Docker process attribution
- WSL and WSLC host attribution trees with an `unattributed` remainder
- Host-wide CPU normalization: all environments use Windows host logical CPUs = 100%
- Interactive terminal UI with flat/tree views, scrolling, and display toggles
- Flat and tree JSON output
- Resource classification as `process`, `container`, `infra`, or internal `host`
- Best-effort degradation when optional WSLC, Docker, or additional-distro collectors are unavailable

## Quick start

```console
cargo build --release
./target/release/wsltop --interactive
```

For a single sample:

```console
./target/release/wsltop --once
```

Run `./target/release/wsltop --help` for the complete option reference.

## Installation

Requirements:

- Windows 11 with WSL2 and Windows interoperability enabled
- A Rust toolchain capable of building the locked dependencies
- PowerShell available as `powershell.exe` from WSL
- Optional: `wslc.exe` for WSL Containers data
- Optional: Docker CLI plus a reachable Docker daemon for Docker data

Build from source:

```console
git clone https://github.com/adachi6k/wsltop.git
cd wsltop
cargo build --release --locked
install -Dm755 target/release/wsltop ~/.local/bin/wsltop
```

## Usage

The default and `--once` modes take two cumulative CPU snapshots separated by `--interval-ms` and print one table.

```console
wsltop [OPTIONS]

--once                 Take one sampled measurement (default)
-i, --interactive      Run the continuously updating terminal UI
--json                 Emit JSON; incompatible with --interactive
--tree                 Show CPU attribution; selects the initial TUI view
--limit N              Limit flat resources (default: 30)
--interval-ms N        Sampling/refresh interval (default: 1000, minimum: 100)
--show-wsl-host        Show raw vmmem/vmmemWSL/vmmemwslc-* rows in flat views
--wsl-only             Skip Windows, additional-distro, and WSLC collectors
--no-wslc              Disable WSLC collection
--no-docker            Disable Docker collection
--hide-infra           Hide infrastructure rows
```

Options that affect collection or the initial view also apply to interactive mode. `--once` remains an explicit alias for the default one-shot behavior.

## Interactive TUI

Start the terminal UI with `wsltop --interactive`. A background worker calls the same in-process monitoring engine as one-shot mode, while the UI thread remains available for keyboard input and drawing. Each completed sample starts the next one immediately; the sampling interval is applied once inside the engine. The TUI does not launch child `wsltop` processes.

Controls:

| Key | Action |
| --- | --- |
| `q`, `Esc` | Quit |
| Up/Down, Page Up/Page Down | Scroll |
| `t` | Toggle flat/tree view |
| `i` | Toggle infrastructure rows |
| `h` | Toggle raw WSL host rows in flat view |
| `0` | Toggle zero-CPU rows |

Terminal raw mode, alternate-screen state, and cursor visibility are restored on normal exit and propagated errors.

## CPU accounting

Every CPU percentage uses one common denominator: all logical CPUs reported by the Windows host equal 100%. On a 16-logical-CPU host, one fully busy CPU is therefore approximately 6.25%, and four fully busy CPUs are approximately 25%.

Windows and WSL process percentages come from deltas of cumulative processor time. WSLC and Docker percentages are normalized from their collector-specific values onto the same host scale. See [CPU accounting](docs/cpu-accounting.md) for formulas and caveats.

## Attribution tree

Use `--tree` to treat `vmmem`, `vmmemWSL`, and `vmmemwslc-*` as parent resources:

```text
Host logical CPUs: 16

WSL VM                                    8.20%
|- infra      plan9                       2.70%
|- process    codex                       0.20%
`- unattributed                           5.30%
```

The remainder is clamped at zero:

```text
unattributed = max(host CPU - known child CPU, 0)
```

Children are never proportionally scaled to fit a parent. If independently timed samples make children exceed the host value, the internal tree records sampling skew. Memory is displayed as resource data only; host-minus-child memory attribution is not performed.

Raw WSL host processes stay hidden in default flat output to avoid accidental double-counting. `--show-wsl-host` exposes them for diagnostics; tree mode always collects them for use as parents.

## Docker / WSLC behavior

WSLC collection uses the current/default CLI session. A single available `vmmemwslc-*` host can be associated with its containers. If multiple hosts make the mapping ambiguous, `wsltop` reports the mapping as unresolved and does not guess; flat WSLC rows remain available.

Docker collection is optional. Container CPU and memory come from Docker statistics, while `docker top` PIDs are used to nest matching current-WSL processes under containers in the attribution tree. PID matching is restricted to resources from the current distribution; a process in another distribution with the same PID is not attributed to the container.

A missing `wslc.exe`, missing Docker CLI, or recognized unavailable Docker daemon is treated as an expected absence: its rows are silently omitted and monitoring continues. Unexpected command, output, parse, or per-container attribution failures are reported through the common warning path. Use `--no-wslc` or `--no-docker` to disable a collector intentionally.

## Multiple WSL distributions

The current distribution is sampled directly from `/proc`. Other running distributions are discovered with `wsl.exe --list --running --quiet`, sampled through `wsl.exe -d`, and labelled with their distribution name. These remote samples are best-effort and introduce more timing skew than direct `/proc` access.

`--wsl-only` intentionally limits collection to the current distribution. It cannot obtain the Windows host CPU count or host resources, so output warns that CPU normalization and host attribution are limited.

## JSON output

`--json` preserves the flat resource-array schema. Each resource includes fields such as `environment`, `kind`, identity, CPU percentage, and memory bytes.

```console
wsltop --once --json
```

`--tree --json` emits a structured object containing `host_logical_cpu_count`, attribution groups, Docker subgroups, unmapped children, `unattributed_cpu_percent`, and sampling-skew information.

JSON is a one-shot interface; `--interactive --json` is rejected explicitly.

## Limitations

- Sampling is best effort. Linux, PowerShell, WSLC, Docker, and remote-distro snapshots are not captured atomically.
- PowerShell process collection adds latency and is not yet a persistent collector.
- WSLC session attribution is deliberately conservative when multiple host mappings are possible.
- Docker process nesting depends on host PID visibility and currently matches processes visible in the current WSL distribution.
- Memory values from Windows, WSLC, and Docker have different meanings and are not attributed by subtraction.
- This is a WSL2-hosted CLI/TUI, not a native Windows executable or GUI.

## Documentation

- [Architecture](docs/architecture.md)
- [CPU accounting](docs/cpu-accounting.md)
- [Validation and test plan](docs/test-plan.md)
- [Changelog](CHANGELOG.md)
- [Contributing](CONTRIBUTING.md)
- [Security policy](SECURITY.md)

## Development

```console
cargo fmt --all -- --check
cargo test --locked --all-targets
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo build --release --locked
```

CI runs portable build, lint, test, and help/version smoke checks on Ubuntu. Windows/WSL interoperability, WSLC, Docker, attribution accuracy, and terminal recovery require real-host validation; see the test plan.

## License

Licensed under the [MIT License](LICENSE).
