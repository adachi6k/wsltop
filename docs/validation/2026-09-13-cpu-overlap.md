# High-load CPU overlap investigation — 2026-09-13

Related: Issue #46, CPU fix PR #47, release preparation #45.
This investigation is read-only; it did not stop the user's build or containers.

## Reproduction

The reported header was host 100.0%, Win 7.6%, WSL 98.4%, Docker 11.9%, WSLC 0.0%.
Adding these category observations gives 117.9%. Their common normalization does
not mean they form disjoint shares of physical CPU.

Independent samples from the corrected binary reproduced the high-load gap:

| Time (JST) | Host | Win | WSL | Docker | Available-category sum |
| --- | --- | --- | --- | --- | --- |
| 12:50:40 | 99.97% | 6.48% | 97.88% | 12.44% | 116.80% |
| 12:50:50 | 99.97% | 5.02% | 98.85% | 11.41% | 115.28% |
| 12:51:01 | 99.95% | 4.44% | 98.92% | 5.83% | 109.19% |

WSLC was unavailable in these MCP samples, not measured zero. Collection calls
lasted 9.0–9.4 seconds with optional collectors. Independent Linux kernel readings
were 98.12%, 98.61%, and 98.58%; the WSL category calculation was close to its source.

## Docker overlap confirmed

`docker info` reported Docker Desktop, Linux, 16 CPUs and the same WSL2 kernel
version. The current distribution and a running Docker container reported the
same `/proc/sys/kernel/random/boot_id`. Thus this Docker WSL2 backend shares the
kernel measured by the WSL category; Docker CPU is included in that category.

This matches [Docker's WSL backend documentation](https://docs.docker.com/desktop/features/wsl/)
and its [kernel-isolation limitations](https://docs.docker.com/enterprise/security/hardened-desktop/enhanced-container-isolation/limitations/).
The earlier blanket description of Docker Desktop as a separate VM was incorrect
for this backend. Shared kernel does not prove shared PID namespaces or a valid
attribution-tree parent; the existing top-level Docker grouping remains appropriate.

## Windows plus WSL still is not a physical partition

Hyper-V performance counters were available to the current Windows user. There
were 16 logical processors, 16 root virtual processors and 32 guest virtual
processors in two groups. Consequently the guest `_Total` average must not be
compared directly with a 16-core host percentage.

For a longer-window comparison, raw per-processor `PercentTotalRunTime` deltas
were divided by their own `PercentTotalRunTime_Base` deltas, multiplied by 100,
summed and divided by 16 host logical CPUs. The counter type was
`PERF_PRECISION_100NS_TIMER` (542573824), whose denominator is the provider's base
timestamp, not the generic system timestamp
([Microsoft counter documentation](https://learn.microsoft.com/en-us/windows/win32/wmisdk/precision-timer-algorithm-counter-types)).

| Time (JST) | Hyper-V physical | Root partition | Guest partitions | Linux kernel |
| --- | --- | --- | --- | --- |
| 13:00:07 | 94.24% | 14.22% | 79.96% | 88.99% |
| 13:00:16 | 91.38% | 12.05% | 79.38% | 86.27% |

The two guest groups contributed approximately 79.90% + 0.06% and 79.33% + 0.05%.
The root/guest sums closely matched the physical total, while Linux busy accounting
was higher than the aggregate guest execution-time observation. These were about
eight-second Linux intervals; CIM classes and `/proc` were read sequentially and
their timestamps are not atomic. The VM IDs were not mapped to distributions, so
the table describes all guest partitions rather than claiming an exact per-VM split.

The result supports the documented distinction between guest OS CPU accounting
and physical execution time. It does not assign every point of the user's earlier
17.9-point excess to a specific scheduler mechanism or measurement window.
Microsoft explicitly warns that root/child OS utilization is not actual physical
CPU usage and recommends Hyper-V counters for physical/partition measurements
([Hyper-V configuration](https://learn.microsoft.com/en-us/windows-server/administration/performance-tuning/role/hyper-v-server/configuration)).

## Consequence for the release

The shared-kernel fix addresses missing short-lived process CPU. It does not make
the categories add to 100%. Retain them as independent observations, explicitly
explain Docker overlap and guest time, and do not subtract or proportionally scale
values to manufacture a partition. A true physical breakdown needs a separate
Hyper-V-based metric and reliable partition mapping; it is not implemented here.

PR #47 review fixes additionally restrict remote system reads to the primary
distribution, make optional counters non-fatal, ensure Windows `--wsl-only` uses
the host count, reject normalization transitions, and correct platform-specific
help. Release #45 remains held until the reviewed fix is merged and the resulting
release commit is validated.
