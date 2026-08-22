# WSL Unified Process Monitor (`wsltop`)

Phase 0 prototype of a unified Windows + WSL CPU process monitor that runs from WSL2.

The core use case is:

> Windows Task Manager says `VmmemWSL` is busy. Which Windows/WSL workload is actually using the host CPU?

## Phase 0

`wsltop` samples the current WSL distro and Windows native processes, normalizes both to the same host-wide CPU scale, merges them, and sorts by CPU usage.

```text
Host logical CPUs: 16
ENV        CPU%       MEM      PID  COMMAND
--------------------------------------------------------------------
WSL       24.10%     2.31G    18931  verilator
Windows   18.20%     3.82G    22104  chrome
WSL       11.80%     1.44G    19302  cc1plus
Windows    7.20%     1.72G    19420  Code
```

## Requirements

- WSL2
- Windows/WSL interop enabled (`powershell.exe` callable from WSL)
- Rust toolchain

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
```

`--show-wsl-host` exposes `vmmem`/`vmmemWSL`, but those rows overlap with WSL process CPU and must not be summed with the WSL rows.

## CPU semantics

The display intentionally uses the Windows Task Manager style host scale:

```text
all host logical processors fully busy = 100%
```

See [`docs/cpu-accounting.md`](docs/cpu-accounting.md) for the exact formula and double-counting rules.

## Roadmap

- [x] Phase 0 design: Windows + current WSL process merge
- [ ] Validate Phase 0 on a real WSL2 host against Task Manager
- [ ] Phase 1: `VmmemWSL` attribution + `unattributed`
- [ ] Phase 2: Docker container stats
- [ ] Phase 3: Docker process attribution
- [ ] Phase 4: multiple WSL distros
- [ ] Phase 5: interactive ratatui TUI
- [ ] Phase 6: optional Windows GUI

## Current limitations

- PowerShell is started once per snapshot in Phase 0; a persistent collector is a later optimization.
- Windows PID reuse is detected only indirectly (negative cumulative CPU delta). Linux uses PID + starttime.
- Docker and other WSL distros are intentionally not included yet.
- CPU accounting still needs real-host validation against Task Manager before the semantics are considered stable.
