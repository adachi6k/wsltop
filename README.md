# WSL Unified Process Monitor (`wsltop`)

`wsltop` is a unified Windows + WSL + WSL Containers CPU monitor that runs from WSL2.

The core use case is:

> Windows Task Manager says WSL is busy. Which Windows, WSL, or WSLC workload is actually using the host CPU?

## Phase 0.2

Phase 0 validated Windows-native and current-WSL process collection on a real 16-logical-CPU WSL2 host. Phase 0.1 added automatic WSL Containers (WSLC) discovery through `wslc.exe stats --format json --no-trunc`. Phase 0.2 classifies every row as `process`, `container`, or `infra`.

All rows use the same host-wide CPU scale:

```text
Host logical CPUs: 16
ENV     TYPE         CPU%       MEM       ID/PID  COMMAND
------------------------------------------------------------------------------------
WSLC    container   6.27%     1.04G  5a489c3faa3d  misty_beartooth
WSLC    container   6.24%     1.04G  986c5523ef5c  mossy_sangre
Windows process     1.08%      141M         31460  Taskmgr
WSL     process     0.10%      296M           311  codex
WSL     infra       0.01%        2M           104  plan9
```

## Requirements

- WSL2
- Windows/WSL interop enabled (`powershell.exe` callable from WSL)
- Rust toolchain
- Optional: WSL 2.9.3+ with `wslc.exe` for WSLC container rows

WSLC is auto-detected. If `wslc.exe` is absent, `wsltop` continues with Windows + WSL only.

The project itself should live in the WSL filesystem (for example `~/src/...`) rather than `/mnt/c/...` for normal Linux build performance.

## Build

```bash
cargo build --release
```

## Run

```bash
./target/release/wsltop --once
```

Useful options:

```bash
./target/release/wsltop --limit 50
./target/release/wsltop --interval-ms 2000
./target/release/wsltop --json
./target/release/wsltop --show-wsl-host
./target/release/wsltop --wsl-only
./target/release/wsltop --no-wslc
./target/release/wsltop --hide-infra
```

`--show-wsl-host` exposes `vmmem`, `vmmemWSL`, and `vmmemwslc-*`. Those rows overlap with WSL/WSLC workloads and must not be summed with their child workloads.

Windows and ordinary WSL processes are `process`, WSLC rows are `container`, and the WSL `plan9` process is `infra`. `init` and `systemd` remain `process`. `--hide-infra` removes `infra` rows from table and JSON output. JSON rows expose the classification in the existing `kind` field, for example `"kind": "infra"`.

## CPU semantics

The display intentionally uses the Windows Task Manager style host scale:

```text
all host logical processors fully busy = 100%
```

For Windows and WSL processes, `wsltop` differences cumulative CPU time. For WSLC, the native `wslc stats` CPU percentage is divided by the Windows logical CPU count.

See [`docs/cpu-accounting.md`](docs/cpu-accounting.md) for the exact formula and double-counting rules.

## Roadmap

- [x] Phase 0: Windows + current WSL process merge
- [x] Validate Phase 0 on a real WSL2 host
- [x] Phase 0.1 design: WSLC JSON schema and host-process recognition
- [x] Phase 0.2: resource type classification and infrastructure filtering
- [ ] Validate Phase 0.1 WSLC CPU normalization against Task Manager
- [ ] Phase 1: WSL/WSLC VM attribution + `unattributed`
- [ ] Phase 2: Docker container stats
- [ ] Phase 3: Docker process attribution
- [ ] Phase 4: multiple WSL distros and WSLC sessions
- [ ] Phase 5: interactive ratatui TUI
- [ ] Phase 6: optional Windows GUI

## Current limitations

- PowerShell is started once per Windows snapshot; a persistent collector is a later optimization.
- Windows PID reuse is detected only indirectly (negative cumulative CPU delta). Linux uses PID + starttime.
- Phase 0.1 reads WSLC containers from the current/default WSLC CLI session only.
- WSLC memory and Windows `vmmemwslc-*` working set are displayed but are not subtracted from each other because they use different accounting semantics.
- Docker and other WSL distros are intentionally not included yet.
