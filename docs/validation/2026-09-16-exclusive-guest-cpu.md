# Verified WSL/container CPU overlap removal

## Change

The two-line CPU/RAM header remains Win / WSL / WSLC / Docker. Supported running
containers provide boot identity, cgroup identity and cumulative cgroup v2 CPU
through a read-only shell probe. A matching WSL kernel boot and dedicated leaf
cgroup establish the supported shared scope. No container is started or modified.

Kernel and cgroup counters are linearly interpolated within bracketing observations
to a common interval. Docker/WSLC use those rates and WSL subtracts them. This is
an estimate over a shared interval, not an atomic measurement or a rescaling to
Windows total. Exact cgroup aliases exposed by both backends count once in Docker.

Unknown membership, unsupported cgroups/images, too many containers, missing or
stale samples, counter resets and negative residuals retain inclusive values.
The header marks `WSL*`; snapshot warnings and MCP `cpu_overlap_unresolved` expose
that state. Memory and individual process/container rows retain their scopes.

## Final release measurement

2026-09-16, 03:49:44–03:50:42 JST: 19 TUI captures at approximately three-second
intervals. All 19 resolved overlap; none showed a collection warning or missing
CPU metric. This was the Linux release binary on the existing WSL2/Windows host.

| Displayed measurement | Mean |
| --- | ---: |
| Host CPU | 47.34% |
| Win | 23.47% |
| WSL, excluding verified containers | 5.78% |
| WSLC | 0.00% |
| Docker | 20.15% |
| Four-column sum | 49.41% |
| Sum minus host CPU | +2.07 percentage points |

The difference ranged from +1.4 to +2.8 points. Cgroup CPU separated from WSL
averaged 20.15 points. The kernel-inclusive estimate can be reconstructed by
adding that value back to WSL; counting it again under Docker would duplicate it.
Windows root and physical CPU still have independent windows/accounting.

An earlier high-load run also resolved 19/19 observations: host 94.10%, Win
30.83%, exclusive WSL 59.64%, WSLC 0%, Docker 13.29%; mean sum-minus-host +9.66
points (range +7.0 to +13.5). Removing container overlap does not guarantee a
small host/guest difference under all workloads. These runs and the original
pre-fix observations used different workloads and are not a controlled benchmark.

## Validation

- `cargo test --locked --all-targets`: 225 passed; four live/environment tests
  ignored by default.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo build --release --locked`: passed.
- `RUSTFLAGS='-D warnings' cargo check --locked --all-targets --target
  x86_64-pc-windows-gnu`: passed.
- Explicit `live_exclusive_guest_cpu`: passed in 9.54 seconds after running
  outside the sandbox's Windows-interop restriction. One-shot WSL 6.10% and Docker
  6.73% were reconciled without warnings. That test uses its own workload window.
- Regression tests cover common-window interpolation, exact aliases, foreign
  kernels, stale/missing data, resets/restarts, negative residuals, malformed
  probes, preserved fallback memory/CPU and `WSL*` within the two-row width limit.

Active Docker containers with supported leaf cgroup v2 were exercised. No active
WSLC workload or Windows-native runtime was available in these validation runs;
those paths share the tested calculation/probe code and Windows compilation passed.
Images without `sh`, `cat`, `stat`, or `awk`, cgroup v1, parent/host cgroups and
more than 16 containers per backend deliberately fall back. Probes have bounded
concurrency/timeouts and add a small amount of measured work inside containers.

The local release executable was rebuilt; the installed executable was not
replaced. Temporary TUI sessions were closed after measurement.
