# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.0] - 2026-09-12

### Added

- Default compact two-line host CPU/RAM summary, including host-wide CPU counter metrics and physical RAM in use/total, independent of row filters and CPU display scale.
- Aligned CPU/RAM history graphs with a fixed 0–100% scale, held values between results and explicit failure markers.
- Colorized Windows/WSL/WSLC/Docker observations and environment labels; help explains overlapping CPU observations and different memory definitions.
- `--header classic` for the traditional one-line header and `--color auto|always|never`, including NO_COLOR and TERM=dumb support.

### Changed

- TUI summary, resource table and single-line footer are organized into distinct areas. Footer groups view, sort, CPU scale and refresh interval alongside key hints.
- Responsive summaries use 23 history columns on wide terminals and 15 on medium terminals; below 80 columns, totals and essential controls take priority. Values and environment columns stay aligned.
- Summary and table separators share a restrained Dim style; separator width follows content rather than filling wide terminals. Wide summaries add spacing between environments.
- Help and tree presentation retain top-like simplicity, consistent environment colors and existing container child grouping.
- Windows refresh periods include query time without overlapping collection calls.

### Fixed

- Windows TUI restores the caller's exact console input/output modes on exit, including flags changed by event handling.
- CPU and RAM from one Windows collection share a history timestamp, preventing slot drift when event processing crosses an interval boundary.
- Help scroll bounds use actual word wrapping so the last lines remain reachable on narrow terminals.
- Classic mode restores its original header fields; long tree commands do not stretch the summary separator, and standalone Docker headings receive their environment color.
- Windows command timeout handling uses Job Objects and bounded output cleanup, closes a completion/timeout race, and covers Windows command-line quoting. Success-path tests tolerate slow CI process startup.

## [0.4.0] - 2026-09-06

### Added

- Windows-native execution is now supported for both the one-shot CLI and interactive TUI.
- Windows-target compile checking, native Windows test execution, and target-specific command timeout implementations as groundwork for native Windows collection.
- A mockable process-snapshot collector boundary and shared `CollectorPlan` selection for current and additional WSL distributions across one-shot and streaming collection.
- Windows-native one-shot collection with `--distro NAME`, default-distro, and running-distro primary selection.
- Windows-native interactive monitoring with independent collector scheduling, partial updates, and terminal restoration.
- Shared CPU, memory, and name sorting with `--sort cpu|memory|name` and `--sort-order asc|desc`; TUI keys `c`, `m`, `n`, and `r` update the displayed order immediately.
- Linux x86_64 and Windows x86_64 MSVC release archives with SHA-256 checksums and extracted-executable verification; release packaging can be validated without publishing a tag.

### Changed

- Linux `/proc` parsing is target-neutral and separate from local filesystem collection; `libc` is now a Unix-only dependency.
- One-shot sampling now fixes the additional WSL collector set once per sample, verifies running status before each optional capture to avoid restarting stopped distributions, and keeps optional discovery and distro failures as warnings.
- Windows-native CLI and TUI use remote WSL snapshots while preserving `source: None` for the primary distro and connecting the existing Windows, WSLC, and Docker collectors.
- CLI/JSON and TUI share `ResourceQuery` sorting and snapshot projections. Child processes remain grouped beneath their containers, with limits applied after sorting and deterministic tie-breaking.
- Memory/name tree views include idle Windows applications and processes. CPU descending remains the default; JSON fields, CPU accounting, and collector-provided memory values are preserved.

### Fixed

- Windows TUI key-release events no longer undo toggle actions such as reverse sorting and tree view.
- WSL distribution matching is case-insensitive, preventing duplicate primary collection and mismatched remote snapshots.
- Optional distro discovery no longer delays initial TUI sampling; additional distro rows remain loading until an interval delta is available.

## [0.3.0] - 2026-08-29

### Added

- Stateful, partial TUI collector updates with a 150 ms current-WSL startup warmup and independent Windows, additional-WSL, WSLC, and Docker scheduling.
- Loading/error status with last-good collector data retained across transient failures.
- `--cpu-scale core|host` for top-style per-core or Task Manager-style whole-host human-readable CPU display.
- Conservative Windows application aggregation with WebView2 ownership evidence and tree-level PID detail.
- Top-style `TIME+` cumulative CPU time for Windows, WSL, Docker, and WSLC processes, plus summed Windows application totals.

### Changed

- The default sampling and TUI refresh interval is 3000 ms, matching the calmer cadence commonly expected from Linux `top`; `--interval-ms` still overrides it.
- Docker and WSLC aggregate collection is separated from lazily requested process detail; optional collectors use a lower interactive cadence and no longer block local rows.
- The Windows host logical CPU count is cached after initial discovery instead of querying CIM for every process snapshot.
- Text and TUI output default to one fully busy logical CPU equaling 100%; internal accounting and JSON remain host-wide.
- Human-readable flat/TUI output ranks Windows applications once while flat JSON preserves PID-level compatibility; interactive metadata discovery is independently scheduled and retains last-good state.
- Resource JSON optionally includes additive `cpu_time_seconds`; unsupported container process backends retain detail rows without TIME+.
- Flat/TUI columns follow top-style process ordering: `ID/PID`, `CPU%`, memory, `TIME+`, then command.
- Single-process Windows application rows show the real PID; multi-process rows show `N PIDs` without changing JSON PID compatibility.
- Docker and WSLC process rows are enabled by default for text/TUI output; `--hide-container-processes` disables them, while flat JSON remains PID-compatible by default.

## [0.2.0] - 2026-08-24

### Added

- Docker-internal process discovery through `docker top`, independent of the invoking WSL distribution's `/proc` and PID namespace.
- WSLC-internal process discovery through `wslc.exe exec` and in-container `ps`.
- Container process CPU, PID, PPID, RSS, command, and argument metadata with Windows-host CPU normalization.
- Container-level `unattributed_cpu_percent` and `over_attributed_cpu_percent` accounting without proportional process scaling.
- Unified `--show-container-processes` and `--container-process-limit` options for optional flat Docker/WSLC process visibility.

### Changed

- Docker Desktop attribution remains an independent top-level group when no valid Docker-host/VM mapping is known; containers are no longer attached to the current WSL VM by numeric PID coincidence.
- Flat output ranks and limits containers by total container CPU, then groups CPU-sorted child processes and residual accounting directly beneath each selected container.
- Long container IDs are shortened to 12 characters in text process labels while complete native IDs remain available in JSON.
- Docker and WSLC ps-style process CPU averages are explicitly distinguished from interval-sampled `/proc` CPU measurements.

### Compatibility

- Default flat output continues to include existing container rows without internal process rows.
- Flat JSON remains a top-level resource array; process metadata is additive and opt-in.
- Tree JSON keeps existing fields and adds WSLC process attribution groups.
- `--show-docker-processes` and `--docker-process-limit` remain accepted as aliases for the unified container options.

## [0.1.0] - 2026-08-23

### Added

- Unified monitoring for Windows native processes, the current and additional running WSL distributions, WSL Containers, and Docker containers.
- Host-wide CPU normalization across all collectors.
- Resource kinds for processes, containers, infrastructure, and internal host resources.
- WSL/WSLC CPU attribution trees with clamped unattributed and sampling-skew values.
- Docker process attribution beneath container resources.
- Flat and structured tree JSON output.
- Interactive terminal UI with scrolling and display toggles.

### Changed

- CLI and TUI now share an in-process monitoring and sampling engine.
- Interactive mode honors collector, interval, limit, filtering, and initial tree options.
- Repository metadata, documentation, validation guidance, CI, and release automation for the initial release.

### Compatibility

- One-shot flat output remains the default.
- `--once` remains accepted.
- Flat JSON remains a top-level resource array.
- Raw WSL host rows remain hidden by default and available through `--show-wsl-host`.

[Unreleased]: https://github.com/adachi6k/wsltop/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/adachi6k/wsltop/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/adachi6k/wsltop/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/adachi6k/wsltop/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/adachi6k/wsltop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/adachi6k/wsltop/releases/tag/v0.1.0
