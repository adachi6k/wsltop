# CPU Accounting

## Canonical display scale

All normal output uses the Windows-host capacity scale:

```text
all host logical CPUs fully busy = 100%
```

This deliberately differs from traditional Linux `top`, where one fully busy logical CPU is normally shown as 100%.

## Formula

For a process sampled twice:

```text
CPU% = delta(process CPU seconds)
       -------------------------- * 100
       delta(wall seconds) * host logical CPUs
```

Example for a 16-logical-CPU Windows host:

```text
process CPU increases by 4.0 s over 1.0 s
CPU% = 4.0 / (1.0 * 16) * 100 = 25%
```

This lets Windows and WSL workloads appear in one sorted list without changing interpretation by environment.

## WSL processor limits

If `.wslconfig` limits WSL to 8 processors on a 16-processor Windows host, fully saturating all WSL processors consumes 50% of host capacity and therefore displays as approximately 50%.

For this reason `wsltop` obtains the denominator from Windows rather than from the WSL-visible CPU count.

`--wsl-only` is a degraded/debug mode: it uses the WSL-visible logical CPU count and warns that the value may not be host-normalized.

## Linux process CPU time

`/proc/<pid>/stat` fields:

- `utime` (14)
- `stime` (15)
- `starttime` (22)
- `rss` (24)

`utime + stime` is converted from clock ticks using `_SC_CLK_TCK`.

The process identity is `(environment, pid, starttime)` so PID reuse does not produce a false CPU spike.

## Windows process CPU time

PowerShell `Get-Process` exposes cumulative process CPU time in seconds through the `CPU` property. The sampler differences this value between snapshots.

Phase 0 uses `(environment, pid)` as the Windows process identity. Protected process metadata can make start-time enumeration unreliable. If cumulative CPU time decreases, the sample is discarded as a likely PID reuse/reset event.

## Idle process

Windows `Idle` must not be treated as a normal busy process. Its CPU time grows while processors are idle, so it is filtered from the Windows collector.

## WSL host process and double counting

A row such as `VmmemWSL` represents aggregate WSL VM consumption. WSL process rows represent work inside that aggregate.

Therefore this is invalid as a flat sum:

```text
VmmemWSL      40%
verilator     20%
cc1plus       10%
```

The 20% and 10% are already part of the 40%.

Phase 0 hides `vmmem`/`vmmemWSL` by default. Phase 1 will introduce a resource-attribution tree such as:

```text
WSL VM              40%
+- known processes  36%
`- unattributed      4%
```

`unattributed` is intentional and may contain Linux kernel work, WSL infrastructure, virtualization overhead, and sampling skew.
