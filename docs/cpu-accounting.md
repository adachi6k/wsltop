# CPU Accounting

`wsltop` calculates and stores every CPU percentage on a common host-wide scale: all logical CPUs on the Windows host together equal 100%.

This matches Task Manager-style whole-machine reasoning and makes Windows, WSL, WSLC, and Docker values comparable. It differs from tools that report one fully occupied CPU as 100% regardless of host CPU count.

Human-readable text and TUI output default to `--cpu-scale core`, which multiplies the stored value by the host logical CPU count so one fully busy logical CPU appears as 100%, like Linux `top`. Values can exceed 100%. `--cpu-scale host` displays the stored whole-host value directly. JSON always remains host-wide for compatibility, and all attribution and residual calculations occur before display conversion.

## Expected values

On a 16-logical-CPU host:

- one fully busy logical CPU is approximately `100 / 16 = 6.25%`
- four fully busy logical CPUs are approximately `4 * 100 / 16 = 25%`
- all logical CPUs fully busy are approximately `100%`

Scheduler effects, collection latency, and workload variation make observed values approximate.

## Windows and WSL processes

Both collectors record cumulative processor time at the beginning and end of the sampling interval. For a matched process:

```text
delta_cpu_seconds = max(after.cpu_time - before.cpu_time, 0)
elapsed_seconds = after.captured_at - before.captured_at

CPU% = delta_cpu_seconds / elapsed_seconds
       / host_logical_cpu_count * 100
```

Windows cumulative time comes from `Win32_PerfRawData_PerfProc_Process.PercentProcessorTime`
(100ns ticks). `Get-Process.CPU` can be access-denied for System, Defender, and
other users' services; those failures are no longer converted to zero. Missing
required counters fail collection. Windows process rates use the provider's
`Timestamp_Sys100NS` delta, independent of PowerShell completion latency.
Process creation time is retained in PID identity and truncated to the same
microsecond precision as `Win32_Process.CreationDate` application metadata.

In WSL-native execution, current-distribution cumulative time is read directly from `/proc/<pid>/stat`, while additional distributions provide equivalent values through `wsl.exe -d` collection. In Windows-native execution, both the selected primary distribution and additional distributions are sampled remotely through `wsl.exe`.

Process rows still require a matching identity in both samples. They are useful
attribution observations, not the source of the header's host CPU partitions.

## Additive host CPU header

The compact CPU header shows **Win + VM + Other = total**:

The next line, `Guest CPU (overlap)`, shows WSL, WSLC and Docker CPU percentages
individually on the whole-host scale. They are independent guest/container
observations, not mutually exclusive parts of VM CPU. Overlapping values are
displayed as measured, without rescaling; unavailable values show N/A. This line
is omitted only when the terminal is too short to reserve three summary rows.

- **Win**: Hyper-V root partition execution, including Windows system work,
  interrupts and processes that exit between observations.
- **VM**: execution in all Hyper-V guest partitions, including WSL, WSLC,
  Docker virtual machines and unrelated VMs. This is not a per-distro reading.
  At 100 columns or wider, the label is `VM (WSL,WSLC,Docker)`; narrower displays
  use `VM`. The parenthesized names are examples, not separately summed values.
- **Other**: physical execution not assigned to the root/guest measurements,
  including hypervisor work.

All three counter sets are read with one PDH query, using language-neutral
[English counter paths](https://learn.microsoft.com/en-us/windows/win32/api/pdh/nf-pdh-pdhaddenglishcounterw).
Physical, root and guest instance deltas use each
counter's precision-timer base and are normalized by the host logical CPU
count. Guest vCPU count is not used as the denominator. Instance changes,
counter resets, missing instances, and root/guest totals exceeding physical
usage invalidate the sample; values are not rescaled to force agreement.
The calculation uses the raw counter's first/second values and checks its
[PDH status](https://learn.microsoft.com/en-us/windows/win32/api/pdh/ns-pdh-pdh_raw_counter).

On hosts without a hypervisor, GetSystemTimes (or the system-wide counter on
multi-group hosts) supplies the total, all assigned to Win. When host partition
counters cannot be collected, the header shows N/A, and the collector can recover
on subsequent valid samples. Host-only totals are not silently substituted for
physical Hyper-V CPU. `--wsl-only` disables the host CPU breakdown.

The displayed one-decimal partitions use largest-remainder rounding so the
displayed numbers add to the displayed total. Stored raw percentages are never
scaled. Filters, process limits and core-style row display do not affect these
whole-host percentages. RAM remains a collection of independent observations.

MCP `get_system_summary` includes `cpu_breakdown` with `total`, `windows`,
`virtual_machines`, and `other`; it is null when unavailable. The existing
`environments` CPU values remain independent process/guest/container observations.

See the [measured investigation and fix](validation/2026-09-15-cpu-accounting-fix.md).

The same cumulative value is exposed as `TIME+` in text/TUI output. It is CPU time consumed, not elapsed wall-clock age, and is formatted as unbounded minutes plus seconds and hundredths (`MM:SS.hh`). Windows application TIME+ sums the currently observed member processes, so it may decrease when a member exits. JSON exposes the underlying value as optional `cpu_time_seconds`.

Negative deltas are treated as process replacement/PID reuse and do not become negative usage. Process identity includes environment, PID, and source where available.

With `--wsl-only`, Windows process collection is skipped. WSL-native execution uses the WSL-visible logical CPU count as a fallback and warns that exact Windows-host normalization cannot be guaranteed. Windows-native execution obtains the Windows logical CPU count from the Windows process itself, but warns that Windows host-process attribution is disabled.

Windows-native streaming also queries the host count with process collection
disabled. If that query fails, sampling reports an error rather than treating an
affinity-limited visible count as authoritative. WSL system deltas require the
same host count across both captures; a transition establishes a new baseline.

## WSL category CPU

The independent WSL MCP CPU observation uses the primary distribution's `/proc/stat`
and `/proc/uptime`, collected once per sample. It covers the shared kernel across
distributions, including short-lived/exited tasks and kernel work that a pair of
process lists cannot account for. Additional distributions are not added again.
This scope also applies with `--wsl-only`, although that option limits process
enumeration to the primary distribution.

```text
busy_ticks = user + nice + system + irq + softirq
WSL_CPU% = delta_busy_ticks / CLK_TCK / delta_uptime
           / host_logical_cpu_count * 100
```

Idle, I/O wait and stolen time are excluded. Guest counters are not added because
user/nice already include them ([Linux /proc documentation](https://www.kernel.org/doc/html/latest/filesystems/proc.html)).
Boot identity, CPU topology, clock tick rate and individual counter deltas must
remain consistent; invalid/missing samples display unavailable, with no fallback
to a partial process sum. WSL RAM still sums observed process RSS. CPU and RAM
availability are independent: either valid metric remains visible when the other
source is missing or incomplete. Process rows,
attribution and one-shot JSON keep their existing accounting.

This is a guest-kernel observation, not a partition of Windows host CPU. Different
sampling windows and host/guest accounting can prevent exact agreement. Docker
or WSLC workloads in the same kernel overlap; workloads in a separate kernel
are outside this observation. Container category CPU still comes from container
statistics, not child-process sums. A container that disappears between samples
can itself be absent from those statistics.

Even after avoiding container double counting, Windows process CPU plus Linux
kernel CPU is not an additive physical-CPU breakdown. Host-core normalization
does not convert guest CPU accounting into hypervisor-measured execution time.
Microsoft recommends Hyper-V logical/virtual processor counters for that purpose
([Hyper-V configuration guidance](https://learn.microsoft.com/en-us/windows-server/administration/performance-tuning/role/hyper-v-server/configuration)).
See the [high-load investigation](validation/2026-09-13-cpu-overlap.md) for measured
examples. wsltop does not scale category values to force their sum to the host total.

## WSLC containers

`wslc.exe stats --format json --no-trunc` reports `CPUPerc` using its own container convention. `wsltop` divides that percentage by the Windows logical CPU count to place it on the common host-wide scale.

The collector's `MemUsage` value is retained as resource metadata but is not used in parent/child subtraction.

WSLC 2.9 does not expose a `top` command, so wsltop executes `ps -eo pid,ppid,pcpu,rss,time,comm,args` inside each running container with `wslc.exe exec`. The injected `ps` process is excluded. Its ps-style `%CPU` is divided by the Windows host logical CPU count, has the same averaging caveat as Docker process CPU, and explains the container internally without being added to the container total. The `time` column supplies process TIME+; wsltop falls back to the older column set when unsupported.

## Docker containers and processes

Docker reports a container CPU percentage in a convention where one fully busy CPU is approximately 100%. `wsltop` divides it by the host logical CPU count so the result shares the Windows host scale.

Docker process discovery separately runs `docker top <container-id> -eo pid,ppid,pcpu,rss,time,comm,args`. Its `pcpu` uses the same one-busy-CPU-is-100% convention and is normalized independently:

```text
docker_process_CPU% = docker_top_pcpu / Windows_host_logical_cpu_count
```

Unlike wsltop's two-snapshot `/proc` measurement, docker-top `%CPU` is a ps-style average over process lifetime (with platform-specific averaging/decay behavior). It is therefore less precisely aligned with the current wsltop sampling interval and container statistics.

The `time` column is cumulative process CPU time and is displayed independently of `%CPU`. Container and residual TIME+ remain unavailable because `docker stats` does not provide a matching cumulative value. Unsupported `time` output degrades to process rows without TIME+.

Process observations explain a container internally and are never added to its value or rescaled to force a match:

```text
unattributed = max(container_CPU% - sum(process_CPU%), 0)
over_attributed = max(sum(process_CPU%) - container_CPU%, 0)
```

Docker Desktop's WSL 2 backend shares the WSL kernel, so container CPU overlaps
the WSL category total. Its Hyper-V backend uses a separate kernel. Kernel sharing
does not establish shared PID namespaces or a verified host/VM attribution parent;
Docker remains a top-level group when that mapping is unknown.
See [Docker's backend documentation](https://docs.docker.com/desktop/features/wsl/).

## Host attribution

Windows `vmmem`, `vmmemWSL`, and `vmmemwslc-*` process CPU values are parent observations. Known WSL or WSLC resources are child observations:

```text
known_children_cpu = sum(child.cpu_percent)
unattributed_cpu = max(host_cpu - known_children_cpu, 0)
over_attributed_cpu = max(known_children_cpu - host_cpu, 0)
```

The two clamped residuals make sampling behavior explicit:

- `unattributed_cpu_percent` represents host CPU not explained by known children.
- `over_attributed_cpu_percent` records the amount by which child observations exceed the host sample.

No proportional scaling is applied to children. A displayed tree is an attribution model, not an additive list to combine with the parent.

## Windows application totals

Human-readable Windows application rows are derived without scaling:

```text
application_CPU = sum(observed member process CPU)
```

Member PIDs shown in tree output explain that total and are not additional utilization. Internal and JSON values remain on the host-wide scale; optional core-style display conversion occurs only after grouping.

## Sampling alignment

Linux `/proc`, PowerShell, remote WSL, WSLC, and Docker snapshots are collected through different interfaces with different latency. Their sampling windows are not atomic or perfectly aligned. Consequently:

- short-lived work may appear only in a parent or child sample
- known children may temporarily exceed a host
- refresh intervals shorter than collector latency may be noisy
- additional distributions can have more skew because they are sampled serially

Attribution is therefore best effort. A longer `--interval-ms` may reduce relative timing noise, but it also lowers temporal resolution.

In the interactive TUI, the primary WSL and Windows collectors retain their own previous cumulative snapshot rather than restarting a complete global sample. This applies to both local WSL and Windows-native remote sampling. During Windows host discovery, non-Windows rows are provisionally normalized with the logical CPU count visible to the executing platform (WSL guest on WSL, Windows on Windows) and the TUI reports that status. Every normalized event and retained container-detail cache records the CPU count it used; when the Windows count differs, provisional cached rows are invalidated and delayed or cached old-scale results are rejected before fresh samples repopulate them. Optional collectors publish independently and their latest successful values are combined. This lowers latency but does not make cross-collector timestamps atomic. One-shot and JSON modes retain the two-snapshot full-sample behavior.

Additional WSL sources retain independent baselines. The first successful snapshot establishes a baseline and does not provide a CPU delta. Startup loading therefore persists until a delta is available, discovery confirms no additional source, or an error is reported. A transient snapshot failure retains the old baseline, so recovery measures elapsed time across the gap; a confirmed stop discards it. A newly discovered or restarted distro must establish a fresh baseline before contributing CPU rows. The primary uses `source: None` on both platforms, while additional sources preserve their distro label to keep identical PIDs distinct.

## Memory is not attributed

Windows `WorkingSet64`, WSLC `MemUsage`, Docker memory statistics, and Linux process resident memory have different scopes and sharing semantics. `wsltop` displays collector-provided memory values but never computes:

```text
host memory - child memory
```

Tree parent/child relationships apply to CPU attribution only.
