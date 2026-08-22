# Architecture (Phase 0)

## Goal

Provide a single WSL command that ranks Windows-native and current-WSL processes by CPU consumption on one comparable scale.

The user question is intentionally simple:

> `VmmemWSL` is busy. Which actual workload is responsible?

Phase 0 validates the cross-OS measurement path before adding Docker, multiple distros, TUI, or resource attribution.

## Components

```text
                    wsltop
                      |
          +-----------+-----------+
          |                       |
          v                       v
  Linux collector         Windows collector
      /proc/*                powershell.exe
          |                       |
          +-----------+-----------+
                      |
                      v
                   sampler
              cumulative CPU delta
                      |
                      v
                  normalizer
         Windows host capacity = 100%
                      |
                      v
                table / JSON
```

### Linux collector

Reads the current distro's `/proc/<pid>/stat` and `/proc/<pid>/cmdline`.

Collected fields:

- PID
- process start time (Linux jiffies, used as process identity)
- user + system CPU time
- resident set size
- executable/command name

### Windows collector

Runs `powershell.exe` through WSL interop and collects `Get-Process` output.

Collected fields:

- PID
- process name
- cumulative CPU time (`CPU`)
- working set
- Windows logical processor count

`Idle` is excluded because its cumulative CPU time represents idle processor time, not busy processor time.

`vmmem` and `vmmemWSL` are hidden by default in Phase 0. Showing those rows together with their WSL workload would double-count the same resource consumption.

### Sampler

Each collector takes two snapshots separated by the configured interval. CPU usage is computed from the delta in cumulative CPU time.

Linux and Windows snapshot timestamps are kept separately so PowerShell startup/collection overhead does not corrupt the sampling interval of the other collector.

## Scope boundaries

Phase 0 intentionally does **not** solve:

- Docker containers/processes
- multiple WSL distributions
- exact `VmmemWSL` attribution
- kernel/virtualization overhead attribution
- disk/network/GPU accounting
- interactive TUI

Those belong to later phases after the fundamental CPU normalization is validated against Windows Task Manager.

## Roadmap

1. Phase 0: current WSL distro + Windows native processes
2. Phase 1: `VmmemWSL` attribution and `unattributed` bucket
3. Phase 2: Docker container statistics
4. Phase 3: Docker process attribution
5. Phase 4: multiple WSL distros
6. Phase 5: ratatui interactive UI
7. Phase 6: optional Windows GUI
