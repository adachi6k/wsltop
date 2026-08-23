# WSL Unified Process Monitor (`wsltop`)

`wsltop` is a unified Windows + WSL + WSL Containers + Docker CPU monitor that runs from WSL2.

The core use case is:

> Windows Task Manager says WSL is busy. Which Windows, WSL, or WSLC workload is actually using the host CPU?

## Phase 5

Phase 5 adds an interactive `top`-style terminal UI while preserving every one-shot, table, tree, and JSON interface. It continuously resamples the unified Windows/WSL/WSLC/Docker view.

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
- Optional: Docker CLI and a reachable daemon for Docker container rows

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
./target/release/wsltop --tree
./target/release/wsltop --tree --json
./target/release/wsltop --no-docker
./target/release/wsltop --interactive
```

`--show-wsl-host` exposes `vmmem`, `vmmemWSL`, and `vmmemwslc-*`. Those rows overlap with WSL/WSLC workloads and must not be summed with their child workloads.

Windows and ordinary WSL processes are `process`, WSLC rows are `container`, and the WSL `plan9` process is `infra`. `init` and `systemd` remain `process`. `--hide-infra` removes `infra` rows from table and JSON output. JSON rows expose the classification in the existing `kind` field, for example `"kind": "infra"`.

`--tree` displays CPU attribution without changing the default flat output:

```text
Host logical CPUs: 16

WSL VM                                      8.20%
|- infra      plan9                         2.70%
|- process    codex                         0.20%
`- unattributed                             5.30%

WSLC session: wslc-cli-adach               13.40%
|- container  frosty_scandinavian           6.03%
|- container  daring_urals                  0.65%
`- unattributed                             6.72%
```

The tree is CPU-only. `unattributed = max(host CPU - known children CPU, 0)`. Children are never proportionally scaled. If child CPU exceeds host CPU because collectors sampled different intervals, the tree reports sampling skew and keeps `unattributed` at zero. `--tree --json` emits a structured object; plain `--json` retains the existing flat resource array.

`--tree --hide-infra` hides infrastructure rows after attribution, so hidden `plan9` CPU remains accounted as a known child rather than being moved into `unattributed`.

The current/default WSLC CLI session is mapped only when exactly one `vmmemwslc-*` host is available. With zero or multiple candidates, wsltop reports the mapping as unresolved instead of guessing; normal flat container output remains available.

Docker is auto-detected through `docker stats --no-stream --no-trunc`. Docker's container CPU percentage is divided by the Windows logical CPU count. Phase 3 uses `docker top -eo pid` to associate host-visible WSL PIDs with each container. Associated processes move below the container in tree output and are not also shown directly below the WSL VM. Use `--no-docker` to disable collection. A missing CLI or unavailable daemon is treated as “no Docker resources”.

### Interactive controls

`wsltop --interactive` (or `-i`) opens the alternate-screen TUI. `q`/Esc exits, arrows and PageUp/PageDown scroll, `t` switches flat/tree view, `i` toggles infrastructure rows, `h` toggles raw WSL host rows, and `0` toggles zero-CPU rows. The selected `--interval-ms` controls sampling. Terminal raw mode and the alternate screen are restored on normal exit and errors. `--interactive --json` is rejected explicitly; use the one-shot JSON interface for machine consumption.

## CPU semantics

The display intentionally uses the Windows Task Manager style host scale:

```text
all host logical processors fully busy = 100%
```

For Windows and WSL processes, `wsltop` differences cumulative CPU time. For WSLC and Docker, the native container CPU percentage is divided by the Windows logical CPU count.

See [`docs/cpu-accounting.md`](docs/cpu-accounting.md) for the exact formula and double-counting rules.

## Roadmap

- [x] Phase 0: Windows + current WSL process merge
- [x] Validate Phase 0 on a real WSL2 host
- [x] Phase 0.1 design: WSLC JSON schema and host-process recognition
- [x] Phase 0.2: resource type classification and infrastructure filtering
- [x] Phase 1: WSL/WSLC CPU host attribution tree
- [ ] Validate Phase 0.1 WSLC CPU normalization against Task Manager
- [x] Phase 2: Docker container stats
- [x] Phase 3: Docker process attribution
- [x] Phase 4: multiple WSL distros and ambiguity-safe WSLC sessions
- [x] Phase 5: interactive ratatui TUI
- [ ] Phase 6: optional Windows GUI
- Future option: native Windows GUI after CLI/TUI stabilization

## Current limitations

- PowerShell is started once per Windows snapshot; collector latency means Phase 1 attribution is best-effort rather than strict accounting.
- Windows PID reuse is detected only indirectly (negative cumulative CPU delta). Linux uses PID + starttime.
- WSLC 2.9.x exposes stats for the current/default CLI session only. If multiple `vmmemwslc-*` hosts exist, mapping remains unresolved rather than guessed.
- Multiple WSLC host candidates are deliberately left unresolved; session identity is not guessed.
- Memory is not attributed. WSLC `MemUsage` and Windows host `WorkingSet64` use different accounting semantics and are never subtracted.
- Docker PID mapping depends on the active daemon exposing host PIDs through `docker top`; other WSL distros are intentionally not included yet.
