# Four-column CPU header validation

The compact header again has two rows, with Win / WSL / WSLC / Docker columns.
Win CPU uses Windows root execution, including interrupts and short-lived tasks,
rather than the process sum. Process collection fixes remain. The API keeps both
the original environment observations and host `cpu_breakdown`.

Primary WSL, additional WSL, WSLC and Docker now subtract collection duration from
their refresh delay, as Windows already did. Primary startup still waits 150 ms
after its first baseline; optional collectors retain a two-second cadence floor.
This reduces interval drift but does not synchronize independent sample windows.

## Live observation

The release TUI was sampled 15 times at three-second intervals on the same WSL2
host used in the earlier investigation. All 15 captures had numeric readings for
all four CPU columns, with CPU and RAM remaining on two rows.

| Displayed measurement | Mean |
| --- | ---: |
| Host CPU | 31.57% |
| Win | 22.99% |
| WSL | 10.07% |
| WSLC | 0.00% |
| Docker | 6.14% |
| Host minus four-column sum | -7.63 percentage points |

The gap ranged from -8.2 to -7.0 points. These are observations of independently
refreshed values, not synchronized CPU intervals. They demonstrate a functioning
display, **not that aggregate error has decreased**. The original investigation
used a different workload/time window and is not a controlled before/after test.

Docker reported Docker Desktop with a WSL2 kernel matching the local release,
through a Unix socket. This supports possible WSL/Docker overlap. Its container
PID was not accessible in the observing distribution's `/proc`, so matching PID
and kernel release alone did not establish shared accounting scope. Current CLI
statistics do not expose matching kernel identity and interval boundaries for a
safe general subtraction. No Docker/WSLC value is subtracted from WSL or assigned
to Win merely to reduce the gap. Remote Docker daemons and separate VMs remain
possible configurations.

## Checks

- `cargo test --locked --all-targets`: 218 passed; three live/environment tests
  ignored by default. Removed the obsolete additive-display rounding test.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: passed.
- `cargo build --release --locked`: passed.
- Header regression covers 80/100/120/150 columns, root CPU instead of process
  CPU, unavailable counters, independent container readings and two-row layout.
- Cadence regression covers fast, slow and overrun collection durations.

The local release executable was rebuilt; the installed executable was not
replaced. Windows-native runtime validation was not performed in this follow-up.
