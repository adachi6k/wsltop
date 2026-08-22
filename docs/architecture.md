# Architecture (Phase 0.2)

## Goal

Provide a single WSL command that ranks Windows-native processes, current-WSL processes, and WSLC containers by CPU consumption on one comparable host-wide scale.

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
             table / JSON
```

### Linux collector

Reads the current distro's `/proc/<pid>/stat` and `/proc/<pid>/cmdline` and records cumulative CPU time, RSS, PID, start time, and executable name.

### Windows collector

Runs `powershell.exe` through WSL interop and collects `Get-Process` output plus the Windows logical processor count.

`Idle` is excluded. `vmmem`, `vmmemWSL`, and `vmmemwslc-*` are hidden by default because they overlap with WSL/WSLC workload rows.

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

### Resource model

Phase 0 used a process-only output model. Phase 0.1 generalized the final row into `ResourceUsage` so a row can represent either a process or a container while preserving `pid` for process consumers.

Phase 0.2 adds a resource type to every row through the JSON `kind` field and the table's `TYPE` column:

- Windows processes: `process`
- ordinary WSL processes: `process`
- WSLC resources: `container`
- the WSL `plan9` process: `infra`

Classification is deliberately narrow: `init`, `systemd`, and other WSL processes remain `process`. `--hide-infra` filters `infra` rows before sorting and applying `--limit`, and affects both table and JSON output. Existing options and existing JSON fields remain unchanged.

## Scope boundaries

Phase 0.1 intentionally does **not** solve:

- attribution of `vmmemWSL` or `vmmemwslc-*` to children
- multiple WSLC sessions
- Docker containers/processes
- multiple WSL distributions
- kernel/virtualization overhead attribution
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
