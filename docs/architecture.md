# Architecture (Phase 4)

## Goal

Provide a single WSL command that ranks Windows-native processes, current-WSL processes, and WSLC containers by CPU consumption on one comparable host-wide scale.

Phase 1 additionally presents WSL and WSLC host processes as CPU attribution trees without double-counting parents and children. Phase 2 adds flat Docker container statistics.

## Components

```text
                         wsltop
                           |
          +----------------+----------------+
          |                |                |
          v                v                v
  Linux collector   Windows collector   WSLC collector
      /proc/*         powershell.exe      wslc.exe
          |                |             stats --json
          |                |                |
          +--------+-------+----------------+
                   |
                   v
              normalizer
      Windows host capacity = 100%
                   |
                   v
        flat table / JSON
        attribution tree / JSON
```

### Linux collector

Reads the current distro's `/proc/<pid>/stat` and `/proc/<pid>/cmdline` and records cumulative CPU time, RSS, PID, start time, and executable name.

### Windows collector

Runs `powershell.exe` through WSL interop and collects `Get-Process` output plus the Windows logical processor count.

`Idle` is excluded. `vmmem`, `vmmemWSL`, and `vmmemwslc-*` are always collected for attribution and classified as `host`. The flat renderer hides them by default because they overlap with WSL/WSLC workload rows; `--show-wsl-host` exposes the raw rows for diagnostics.

### WSLC collector

Runs:

```text
wslc.exe stats --format json --no-trunc
```

The JSON schema observed with WSLC 2.9.4 contains `ID`, `Name`, `CPUPerc`, `MemUsage`, `NetIO`, `BlockIO`, and `PIDs`.

Phase 0.1 uses:

- full container ID
- container name
- `CPUPerc`
- used-memory portion of `MemUsage`

The WSLC collector is optional. If `wslc.exe` is not installed, monitoring continues with Windows + WSL only.

### Docker collector

Runs `docker stats --no-stream --no-trunc --format '{{json .}}'` and reads one JSON object per running container. `ID`, `Name`, `CPUPerc`, and the used portion of `MemUsage` become `Docker`/`container` resources. CPU is normalized by the Windows logical CPU count. A missing CLI or unavailable daemon quietly returns no rows; unexpected command or data failures warn without stopping other collectors. `--no-docker` disables collection.

Phase 3 runs `docker top <id> -eo pid` for each collected container. Host PIDs are matched to current-WSL process resources. Matched processes are removed from the WSL VM's direct children and nested below their Docker container; the container CPU replaces those process values in the WSL parent's known-child sum. Each container gets its own clamped `unattributed` and sampling-skew values. Flat output is unchanged.

### Resource model

Phase 0 used a process-only output model. Phase 0.1 generalized the final row into `ResourceUsage` so a row can represent either a process or a container while preserving `pid` for process consumers.

Phase 0.2 adds a resource type to every row through the JSON `kind` field and the table's `TYPE` column:

- Windows processes: `process`
- ordinary WSL processes: `process`
- WSLC resources: `container`
- the WSL `plan9` process: `infra`

Classification is deliberately narrow: `init`, `systemd`, and other WSL processes remain `process`. `--hide-infra` filters `infra` rows before sorting and applying `--limit`, and affects both table and JSON output. Existing options and existing JSON fields remain unchanged.

### Attribution model

`src/attribution.rs` owns parent/child mapping and CPU arithmetic so future multi-distro/session work does not depend on CLI rendering code. Each group records:

- the raw Windows host resource
- mapped child resources
- summed known-child CPU
- `unattributed_cpu_percent`
- `over_attributed_cpu_percent` for sampling skew
- `mapping_status` (`resolved` or `unresolved`)

The current distro's WSL resources are mapped to a unique `vmmem` or `vmmemWSL` host. Current/default CLI-session containers are mapped only when exactly one `vmmemwslc-*` host exists. Ambiguous mappings are never guessed: host groups are marked unresolved and children remain in `unmapped_children`.

The calculation is:

```text
known = sum(child CPU%)
unattributed = max(host CPU% - known, 0)
over_attributed = max(known - host CPU%, 0)
```

Children are not rescaled. Linux snapshots, PowerShell snapshots, and `wslc stats` do not share exact sampling boundaries, so this is best-effort attribution. `over_attributed` makes that skew observable without manufacturing adjusted workload values.

With `--tree --hide-infra`, infrastructure child rows are suppressed only after attribution is calculated; their CPU remains part of `known_children_cpu_percent` and is not incorrectly moved into `unattributed`.

`--tree` renders the model as text. `--tree --json` serializes the structured model. Plain `--json` remains the Phase 0.2 flat array for compatibility.

Memory is intentionally absent from attribution arithmetic. Windows `WorkingSet64` and WSLC `MemUsage` are retained only on raw resources and are never subtracted.

## Scope boundaries

### Multiple WSL distributions

Phase 4 enumerates running distributions with `wsl.exe --list --running --quiet`. The current distribution continues to use direct `/proc`; each additional distribution is sampled with `wsl.exe -d <name>` and tagged through the optional `source` field. Process identity includes this source, preventing identical PIDs across distributions from colliding. Remote collection is best-effort and adds timing skew.

WSLC stats remain limited to the current/default CLI session. Multiple `vmmemwslc-*` hosts are represented but containers are attached only with a unique mapping; ambiguity remains explicit and unresolved.

Phase 4 intentionally does **not** solve:

- Docker processes that cannot be exposed as host PIDs by the active daemon
- explanation of the components inside the `unattributed` bucket
- disk/network/GPU accounting
- interactive TUI

## Roadmap

1. Phase 0: current WSL distro + Windows native processes
2. Phase 0.1: current WSLC CLI-session container statistics
3. Phase 0.2: resource type classification and infrastructure filtering
4. Phase 1: WSL/WSLC host attribution and `unattributed` buckets
5. Phase 2: Docker container statistics
6. Phase 3: Docker process attribution
7. Phase 4: multiple WSL distros and WSLC sessions
8. Phase 5: ratatui interactive UI
9. Phase 6: optional Windows GUI
