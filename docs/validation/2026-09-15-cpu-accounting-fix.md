# Host CPU accounting fix — 2026-09-15

## Reproduction and cause

A five-minute observation of wsltop 0.5.2, sampled every ten seconds, found the
host CPU above Win + WSL in all 31 observations. The average values were 19.58%
for the host and 9.29% for Win + WSL, a 10.29-point gap.

Further measurements on the same 16-logical-CPU Windows/WSL2 host established:

- `Get-Process.CPU` was null for 144 of 349 processes, including Idle. Explicitly
  calling System's CPU-time getter returned Windows error 5, `Access is denied`.
  The collector converted these failures to zero. Performance counters retrieved
  those processes' CPU without elevating the Windows user.
- In seven subsequent intervals, omitted non-VM processes contributed an average
  2.993% CPU, including Defender at 1.207% and System at 0.992%.
- Comparing roughly 100ms and 3.4s process snapshots over another 67-second
  interval recovered 1.4755 points of short-lived process CPU; PowerShell
  accounted for 1.2494 points. Adding the high-frequency process observations,
  DPC and interrupt CPU left a 0.4855-point difference from GetSystemTimes.
- VM-host process CPU, Linux guest CPU and physical Hyper-V CPU were different
  observations. Substituting guest CPU for the VM-host observation did not
  produce an additive host breakdown.

These measurements were taken in different workload windows and must not be
added together to explain the original screenshot's exact difference.

## Implemented correction

The CPU header now shows **Win + VM + Other = total**. On Hyper-V systems,
physical, root and guest processor counters come from one PDH query. Separate
CIM queries initially produced intermittent inconsistent samples; one PDH query
eliminated that instability in the validation run. All guest vCPUs are summed
and normalized to the host's logical CPU count.

Win includes root-partition system work and short-lived processes. VM covers
all guest partitions, including WSL, WSLC, Docker VMs and unrelated VMs. Other
is physical execution not assigned to root/guest counters. Missing or invalid
partitions show N/A rather than fabricated or rescaled values. On a host without
a hypervisor, all measured host CPU is assigned to Win.

Process CPU now uses raw PerfProc time and the provider's own timestamp.
Creation times are reduced to CIM metadata's microsecond precision so existing
application grouping can still match process identity. The header's display
rounding preserves additivity without changing stored observations.

MCP exposes the same `cpu_breakdown`; independent environment observations,
process/container rows and memory accounting retain their separate scopes.

## Validation

- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo test --locked --all-targets`: 218 passed; three environment-dependent
  tests ignored by the normal invocation.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `RUSTFLAGS='-D warnings' cargo check --locked --all-targets --target x86_64-pc-windows-gnu`:
  passed.
- `cargo build --release --locked`: passed.
- `cargo package --locked --allow-dirty --offline`: package verification passed.
- `cargo test --locked live_windows_cpu_counters -- --ignored --nocapture`:
  passed on the live host. All 12 intervals produced valid partitions and additive
  displayed values, spanning roughly 14.5–50.6% total CPU. System and Defender
  had measured nonzero CPU; process creation times matched application metadata.
- Release TUI example: `CPU 18.0% | Win 16.5% VM 1.4% Other 0.1%`.

Live testing used the Linux executable inside WSL2 and non-elevated Windows
performance-counter access, including Japanese Windows counter localization.
Windows-native executable execution was not tested locally; the Windows target
was compile-checked. PowerShell still starts per collection; persistent
collection was not required to correct the aggregate CPU accounting.

See [CPU accounting](../cpu-accounting.md) for the measurement contract and
counter documentation. Raw local process/screen captures are not published.
