# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Stateful, partial TUI collector updates with a 150 ms current-WSL startup warmup and independent Windows, additional-WSL, WSLC, and Docker scheduling.
- Loading/error status with last-good collector data retained across transient failures.
- `--cpu-scale core|host` for top-style per-core or Task Manager-style whole-host human-readable CPU display.
- Conservative Windows application aggregation with WebView2 ownership evidence and tree-level PID detail.

### Changed

- The default sampling and TUI refresh interval is 3000 ms, matching the calmer cadence commonly expected from Linux `top`; `--interval-ms` still overrides it.
- Docker and WSLC aggregate collection is separated from lazily requested process detail; optional collectors use a lower interactive cadence and no longer block local rows.
- The Windows host logical CPU count is cached after initial discovery instead of querying CIM for every process snapshot.
- Text and TUI output default to one fully busy logical CPU equaling 100%; internal accounting and JSON remain host-wide.
- Human-readable flat/TUI output ranks Windows applications once while flat JSON preserves PID-level compatibility; interactive metadata discovery is independently scheduled and retains last-good state.

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

[Unreleased]: https://github.com/adachi6k/wsltop/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/adachi6k/wsltop/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/adachi6k/wsltop/releases/tag/v0.1.0
