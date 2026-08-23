# Architecture

`wsltop` separates data acquisition, sampling/accounting, attribution, and presentation. The one-shot CLI and interactive TUI share the same monitoring engine and unified snapshot.

```text
Linux /proc    Windows PowerShell    WSLC CLI    Docker CLI    wsl.exe
     \                 |                |            |           /
      +----------------+----------------+------------+----------+
                               collectors
                                   |
                      Monitor / sampling engine
                                   |
                         unified MonitorSnapshot
                                   |
                        CPU attribution model
                            /              \
                     CLI renderer       TUI renderer
```

## Module responsibilities

| Module | Responsibility |
| --- | --- |
| `linux.rs` | Snapshot processes from the current distribution's `/proc` |
| `windows.rs` | Snapshot Windows process cumulative CPU time and working sets through PowerShell |
| `multiwsl.rs` | Discover and snapshot additional running WSL distributions |
| `wslc.rs` | Collect current/default WSLC container statistics and in-container process observations |
| `docker.rs` | Collect Docker statistics and Docker-namespace process observations |
| `sampler.rs` | Convert cumulative process-time deltas into host-normalized `ResourceUsage` values |
| `monitor.rs` | Orchestrate collectors, sampling interval, degradation warnings, flat filtering, and snapshot construction |
| `attribution.rs` | Build WSL, WSLC, and Docker CPU attribution groups without double-counting |
| `render.rs` | Render a `MonitorSnapshot` as the flat table or text tree |
| `tui.rs` | Own terminal lifecycle, navigation, toggles, and refresh scheduling |
| `main.rs` | Parse/validate CLI options and choose JSON, text, or interactive presentation |

Collector logic is not duplicated between interfaces. `Monitor::sample()` is the only orchestration path used by the CLI and TUI, and the TUI refreshes it directly in-process.

## Sampling flow

`MonitorConfig` carries the interval, flat limit, collector switches, and initial filtering choices. A sample proceeds as follows:

1. Capture the current `/proc`, additional-distro, and Windows cumulative snapshots.
2. Wait for the configured sampling interval.
3. Capture the corresponding second snapshots.
4. Convert matched cumulative-time deltas to host-normalized CPU percentages.
5. Collect WSLC and Docker point-in-time statistics when enabled.
6. Build the attribution tree from raw host and child resources.
7. Prepare the filtered, sorted, limited flat resource list.
8. Return both views plus warnings in a `MonitorSnapshot`.

The engine retains raw Windows WSL-host rows long enough to build attribution even when those rows are hidden from flat output.

## Unified resource model

`ResourceUsage` is the common row type. It records:

- environment: Windows, WSL, WSLC, or Docker
- resource kind: `process`, `container`, `infra`, or `host`
- stable collector identity and optional PID/PPID
- display name, optional arguments, and optional source distribution/container
- host-normalized CPU percentage
- collector-provided memory bytes

Classification is intentionally narrow. WSL `plan9` is infrastructure; ordinary WSL processes, including `init` and `systemd`, remain processes. Windows `vmmem`, `vmmemWSL`, and `vmmemwslc-*` processes are host resources used by attribution and hidden from the default flat view.

Additional distributions are labelled through `source`. Process matching includes the source so identical PIDs in separate distributions do not collide.

## CPU normalization

All environments share a Windows host-wide CPU scale where all logical CPUs together equal 100%. For process snapshots:

```text
CPU% = delta cumulative CPU seconds / elapsed wall seconds
       / Windows logical CPU count * 100
```

WSLC and Docker percentages are normalized from their source conventions to the same denominator. Full formulas are in [CPU accounting](cpu-accounting.md).

## Attribution

Attribution treats Windows WSL VM/session processes as parent observations and known WSL, WSLC, or Docker workloads as children:

```text
host CPU = known child CPU + unattributed CPU
unattributed CPU = max(host CPU - known child CPU, 0)
```

If child samples exceed the parent, attribution records the excess as `over_attributed_cpu_percent` while leaving `unattributed_cpu_percent` at zero. Children are not scaled to force equality. The values remain best-effort because collectors have different latency and snapshot times.

Docker process attribution is owned by the Docker collector and does not depend on the current WSL `/proc`. It parses `docker top <id> -eo pid,ppid,pcpu,rss,comm,args`, normalizes process CPU, and nests those native observations beneath the corresponding container. Docker remains an independent top-level group unless a collector positively proves a Docker-host/VM relationship. In particular, Docker Desktop's VM is not automatically attached to the current WSL VM. The legacy WSL PID-matching path remains available only for a proven shared host PID namespace; an equal numeric PID is not proof.

The Docker process source is intentionally replaceable: attribution consumes per-container process observations, not `docker top` itself. A future collector can provide two snapshots of container `/proc` cumulative CPU time for interval-aligned measurements, with `docker top` retained as fallback.

WSLC containers map to one `vmmemwslc-*` host only when the association is unambiguous. Multiple possible hosts produce an unresolved mapping and ungrouped children; the implementation does not guess.

Memory attribution is deliberately absent because Windows working set, WSLC memory usage, and Docker memory statistics are not interchangeable accounting measures.

## Output paths and compatibility

Flat text and flat JSON consume `MonitorSnapshot.resources`. Host resources are hidden unless `--show-wsl-host` is set; Docker and WSLC process rows are added only with `--show-container-processes` (the old Docker-specific name remains an alias); `--hide-infra`, sorting, and `--limit` are applied by the engine. Containers participate in top-level sorting and limiting by their total CPU value, then up to `--container-process-limit` CPU-sorted process rows are placed directly after the selected container without counting toward `--limit`. Text output summarizes omitted process count and CPU; residual accounting still uses every observed process. Existing container rows are preserved.

Tree text and tree JSON consume `MonitorSnapshot.tree`. Tree mode uses host rows internally regardless of `--show-wsl-host`. Plain `--json` remains a flat resource array for compatibility; `--tree --json` is a separate structured schema.

The TUI renders the same text views from the same snapshot. A background sampling worker immediately begins the next sample after the previous one completes, so `Monitor::sample()` is the only source of the configured interval and the event/render thread remains responsive. Its `t`, `i`, and `h` keys change view/filter state, while collection switches and limits supplied on the command line remain active for the session.

## Degradation and lifecycle

The current WSL `/proc` collector and, unless `--wsl-only` is used, Windows host collection are required for a sample. Optional collectors degrade independently:

- additional-distribution failure: continue with current WSL and other sources
- missing WSLC executable: silently continue without WSLC rows
- missing Docker CLI or recognized daemon-unavailable error: silently continue without Docker rows
- unexpected optional-collector failure: continue without affected data and surface a warning
- ambiguous WSLC hosts: preserve flat rows and mark tree mapping unresolved

Warnings are written to stderr in one-shot mode and surfaced in TUI status. `TerminalGuard` restores raw mode, the alternate screen, and cursor visibility when the TUI exits or unwinds through an error.

## Known boundaries

- Snapshots across collectors are not atomic.
- Windows collection starts a PowerShell process for each cumulative snapshot.
- Additional distributions are sampled serially through `wsl.exe` and have extra skew.
- WSLC mapping covers the current/default CLI session conservatively.
- `docker top` process CPU is a ps-style average rather than an interval sample.
- GUI presentation is outside the current CLI/TUI architecture.
