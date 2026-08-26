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
| `windows_app.rs` | Conservatively group Windows PIDs using executable, parent, command-line, and package evidence |
| `multiwsl.rs` | Discover and snapshot additional running WSL distributions |
| `wslc.rs` | Collect current/default WSLC container statistics and in-container process observations |
| `docker.rs` | Collect Docker statistics and Docker-namespace process observations |
| `sampler.rs` | Convert cumulative process-time deltas into host-normalized `ResourceUsage` values |
| `monitor.rs` | Orchestrate collectors, sampling interval, degradation warnings, flat filtering, and snapshot construction |
| `stream.rs` | Run stateful, independently scheduled TUI collectors and aggregate partial events |
| `attribution.rs` | Build WSL, WSLC, and Docker CPU attribution groups without double-counting |
| `render.rs` | Render a `MonitorSnapshot` as the flat table or text tree |
| `tui.rs` | Own terminal lifecycle, navigation, toggles, and refresh scheduling |
| `main.rs` | Parse/validate CLI options and choose JSON, text, or interactive presentation |

Collector parsing and accounting are shared between interfaces. `Monitor::sample()` preserves atomic one-shot and JSON behavior. The TUI uses the stateful streaming orchestrator, which publishes the same `MonitorSnapshot` type after each collector event.

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

### Interactive streaming flow

The TUI draws an empty loading frame immediately. Independent workers retain their prior cumulative snapshot and emit collector-level updates:

1. Current WSL samples `/proc` after a fixed 150 ms startup warmup, then at the normal interval. It retries an unavailable initial baseline. Non-Windows collectors publish independently using a clearly marked provisional WSL-visible CPU count until host discovery completes.
2. Windows samples cumulative process time independently at the normal interval. Its first snapshot publishes the host CPU count returned by the collector script (CIM first, then `[Environment]::ProcessorCount` fallback). Each normalized collector event carries the CPU count it used. If the published Windows count differs from the provisional count, cached non-Windows rows are invalidated and delayed old-scale events are rejected before later samples repopulate them. The first successful CIM value is cached; failed initial snapshots are retried.
3. Additional WSL, WSLC, and Docker use a minimum two-second cadence.
4. WSLC and Docker publish aggregate container statistics separately from optional process detail. Details run only when tree view or `--show-container-processes` requests them; aggregate refreshes retain same-scale last-good details, and failed per-container detail commands do not erase them. Detail caches carry their normalization CPU count, while aggregate and detail warnings remain separate so an aggregate success cannot hide an unresolved detail failure.
5. Detail requests use a separate capacity-one queue per runtime. Per-container commands run in bounded batches of at most four workers and time out after five seconds, so detail latency cannot stall aggregate cadence or create an unbounded backlog.
6. The aggregator replaces only the source named by an aggregate event, rebuilds both views, and publishes a partial snapshot. Delayed detail events merge process lists only into containers still present in the latest aggregate and never clear aggregate errors or roll back CPU/memory. An error records status but retains that source's last successful data.
7. Windows application metadata runs in its own bounded worker. CPU events never wait for CIM metadata; the aggregator combines current PID CPU with the last successful metadata snapshot.

Collector threads do not wait for one another. This makes startup and refresh latency depend on each visible source rather than the slowest source. The event and aggregate boundary also permits future bounded per-container detail workers without changing attribution or rendering.

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

The resource model and attribution tree always retain this host-wide scale. Human-readable rendering applies the selected `core` or `host` display multiplier at the final formatting boundary; it does not mutate snapshots, ordering, residual calculations, or JSON serialization.

Windows application totals are also derived after sampling by summing unchanged PID observations. Human-readable flat output ranks the derived application once, tree output exposes its member PIDs, and flat JSON continues serializing the separately retained PID-level resource list. WebView2 parent traversal requires the parent PID and executable identity to agree with the current CPU snapshot; stale or ambiguous metadata falls back without manufacturing ownership.

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

The TUI renders the same text views from incrementally rebuilt snapshots. Its `t`, `i`, and `h` keys change view/filter state; `t` also enables or disables expensive container details unless flat details were explicitly requested. Collection switches and limits supplied on the command line remain active for the session.

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
