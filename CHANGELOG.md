# Changelog

All notable changes to this project will be documented in this file. The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/adachi6k/wsltop/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/adachi6k/wsltop/releases/tag/v0.1.0
