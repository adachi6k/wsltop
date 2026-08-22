# CPU Accounting

## Canonical display scale

All normal output uses the Windows-host capacity scale:

```text
all host logical CPUs fully busy = 100%
```

This deliberately differs from traditional Linux/container tooling where one fully busy logical CPU is commonly shown as 100%.

## Windows and WSL process formula

For a process sampled twice:

```text
CPU% = delta(process CPU seconds)
       -------------------------- * 100
       delta(wall seconds) * host logical CPUs
```

Example for a 16-logical-CPU Windows host:

```text
process CPU increases by 1.0 s over 1.0 s
CPU% = 1.0 / (1.0 * 16) * 100 = 6.25%
```

## WSLC container formula

`wslc stats` reports a native CPU percentage such as `100.29%`. Phase 0.1 interprets this using the common container convention where one fully busy logical CPU is approximately 100%, then normalizes it to the Windows-host scale:

```text
wsltop WSLC CPU% = wslc CPUPerc / host logical CPUs
```

For a 16-logical-CPU host:

```text
wslc CPUPerc = 100.29%
wsltop CPU%  = 100.29 / 16 = 6.27%
```

This assumption must be validated against Windows Task Manager and the corresponding `vmmemwslc-*` host process before it is considered stable.

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

Windows process identity is currently `(environment, pid)`. If cumulative CPU time decreases, the sample is discarded as a likely PID reuse/reset event.

## Idle process

Windows `Idle` must not be treated as a normal busy process. Its CPU time grows while processors are idle, so it is filtered from the Windows collector.

## WSL/WSLC host processes and double counting

Rows such as `VmmemWSL` and `vmmemwslc-*` represent aggregate VM/session consumption. WSL process and WSLC container rows represent work inside those aggregates.

Therefore host rows and child workload rows must not be flat-summed.

Phase 0.1 hides these host rows by default. Phase 1 will introduce resource-attribution trees with an explicit `unattributed` bucket for kernel work, infrastructure, virtualization overhead, and sampling skew.

## Memory accounting

Windows `WorkingSet64` and WSLC `MemUsage` do not necessarily have matching accounting semantics. Phase 0.1 displays both but does not subtract container memory from `vmmemwslc-*` working set or label the difference as overhead.
