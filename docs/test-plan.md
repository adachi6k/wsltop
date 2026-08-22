# Phase 0 Validation Plan

The goal is to validate semantics against a real WSL2 + Windows host before adding Docker or a TUI.

## 1. Environment sanity

From WSL:

```bash
powershell.exe -NoProfile -NonInteractive -Command '[Environment]::ProcessorCount'
echo "$WSL_DISTRO_NAME"
rustc --version
cargo --version
```

`powershell.exe` must run successfully. If it does not, check WSL interop before debugging `wsltop`.

## 2. Build

```bash
cargo build --release
./target/release/wsltop --help
```

## 3. Baseline sample

```bash
./target/release/wsltop --once --limit 30
```

Verify that both `Windows` and `WSL` rows appear and that `Idle` and `VmmemWSL` do not appear by default.

Then verify the diagnostic mode:

```bash
./target/release/wsltop --once --show-wsl-host
```

`vmmem`/`vmmemWSL` may now appear; its CPU must not be added to the WSL process rows.

## 4. Single WSL CPU load

Start one busy Linux process:

```bash
yes > /dev/null &
LOAD_PID=$!
./target/release/wsltop --once --limit 20
kill "$LOAD_PID"
```

Expected host-normalized result:

```text
approximately 100 / Windows-host-logical-CPU-count percent
```

For example, one saturated logical CPU on a 16-logical-CPU Windows host should be about 6.25%.

Traditional Linux `top` will normally show that same process near 100%; this difference is intentional.

## 5. Multi-core WSL load

Start four independent busy processes:

```bash
for i in 1 2 3 4; do yes > /dev/null & echo $!; done > /tmp/wsltop-load-pids
./target/release/wsltop --once --limit 30
xargs -r kill < /tmp/wsltop-load-pids
rm -f /tmp/wsltop-load-pids
```

On a 16-logical-CPU Windows host, their total should be near 25% of host CPU capacity (subject to scheduler/sample noise).

Compare the total with Windows Task Manager's overall CPU change and with the WSL VM consumption.

## 6. Sampling stability

Compare several intervals:

```bash
./target/release/wsltop --once --interval-ms 500
./target/release/wsltop --once --interval-ms 1000
./target/release/wsltop --once --interval-ms 2000
```

A longer interval should reduce jitter. If Windows rows vary much more than WSL rows, PowerShell collection timing is the first suspect.

## 7. JSON contract

```bash
./target/release/wsltop --json | jq '.[0:5]'
```

Fields expected per row:

- `environment`
- `pid`
- `name`
- `cpu_percent`
- `memory_bytes`

## 8. WSL CPU-limit test (optional but important)

If Windows has 16 logical CPUs and `.wslconfig` contains:

```ini
[wsl2]
processors=4
```

then fully saturating all four WSL CPUs should display about 25%, not 100%, because the canonical scale is Windows host capacity.

After changing `.wslconfig`, run `wsl.exe --shutdown` from Windows before retesting.

## Exit criteria for Phase 0

Phase 0 is accepted when:

1. Windows and current-WSL processes merge into one CPU-sorted list.
2. One saturated WSL CPU matches `100 / host logical CPUs` within normal sample error.
3. Multi-core load scales approximately linearly.
4. Windows process CPU ranking is directionally consistent with Task Manager.
5. `vmmem`/`vmmemWSL` is hidden by default to avoid double counting.
6. No obvious CPU spikes are caused by PID reuse.

Only after these checks should Phase 1 implement WSL VM attribution and the `unattributed` bucket.
