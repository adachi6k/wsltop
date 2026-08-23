# CPU Accounting

`wsltop` reports every CPU percentage on a common host-wide scale: all logical CPUs on the Windows host together equal 100%.

This matches Task Manager-style whole-machine reasoning and makes Windows, WSL, WSLC, and Docker values comparable. It differs from tools that report one fully occupied CPU as 100% regardless of host CPU count.

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

Windows cumulative time comes from `Get-Process .CPU`. Current-WSL cumulative time comes from `/proc/<pid>/stat`; additional distributions provide equivalent values through `wsl.exe -d` collection.

Negative deltas are treated as process replacement/PID reuse and do not become negative usage. Process identity includes environment, PID, and source where available.

With `--wsl-only`, Windows collection is skipped and the WSL-visible logical CPU count is used as a fallback. The command warns because exact Windows-host normalization cannot be guaranteed in that mode.

## WSLC containers

`wslc.exe stats --format json --no-trunc` reports `CPUPerc` using its own container convention. `wsltop` divides that percentage by the Windows logical CPU count to place it on the common host-wide scale.

The collector's `MemUsage` value is retained as resource metadata but is not used in parent/child subtraction.

WSLC 2.9 does not expose a `top` command, so wsltop executes `ps -eo pid,ppid,pcpu,rss,comm,args` inside each running container with `wslc.exe exec`. The injected `ps` process is excluded. Its ps-style `%CPU` is divided by the Windows host logical CPU count, has the same averaging caveat as Docker process CPU, and explains the container internally without being added to the container total.

## Docker containers and processes

Docker reports a container CPU percentage in a convention where one fully busy CPU is approximately 100%. `wsltop` divides it by the host logical CPU count so the result shares the Windows host scale.

Docker process discovery separately runs `docker top <container-id> -eo pid,ppid,pcpu,rss,comm,args`. Its `pcpu` uses the same one-busy-CPU-is-100% convention and is normalized independently:

```text
docker_process_CPU% = docker_top_pcpu / Windows_host_logical_cpu_count
```

Unlike wsltop's two-snapshot `/proc` measurement, docker-top `%CPU` is a ps-style average over process lifetime (with platform-specific averaging/decay behavior). It is therefore less precisely aligned with the current wsltop sampling interval and container statistics.

Process observations explain a container internally and are never added to its value or rescaled to force a match:

```text
unattributed = max(container_CPU% - sum(process_CPU%), 0)
over_attributed = max(sum(process_CPU%) - container_CPU%, 0)
```

Docker is not charged to the current WSL host unless the Docker daemon is proven to share its host PID namespace. Docker Desktop normally uses a separate Linux VM, so its attribution group remains top-level when no valid host/VM mapping is known.

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

## Sampling alignment

Linux `/proc`, PowerShell, remote WSL, WSLC, and Docker snapshots are collected through different interfaces with different latency. Their sampling windows are not atomic or perfectly aligned. Consequently:

- short-lived work may appear only in a parent or child sample
- known children may temporarily exceed a host
- refresh intervals shorter than collector latency may be noisy
- additional distributions can have more skew because they are sampled serially

Attribution is therefore best effort. A longer `--interval-ms` may reduce relative timing noise, but it also lowers temporal resolution.

## Memory is not attributed

Windows `WorkingSet64`, WSLC `MemUsage`, Docker memory statistics, and Linux process resident memory have different scopes and sharing semantics. `wsltop` displays collector-provided memory values but never computes:

```text
host memory - child memory
```

Tree parent/child relationships apply to CPU attribution only.
