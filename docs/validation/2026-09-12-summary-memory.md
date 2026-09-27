# Issue #29: Windows RAM collector runtime validation

Validated the new PowerShell/P/Invoke path on a real Windows 11 + WSL2 host, using the `snapshot_script` embedded in `src/windows.rs` on `feat/compact-resource-summary` (PR #30 review revision).

## Environment

- Windows 11 Pro, version 10.0.26200; WSL reports OS build 26200.9445.
- Windows PowerShell 5.1.26100.9444 (`powershell.exe`).
- WSL 2.9.4.0; running kernel `6.18.35.2-microsoft-standard-WSL2`.
- Ubuntu 24.04.4 LTS, 16 host logical CPUs, wsltop 0.4.0 development branch.

## Procedure and results

Extracted the exact raw script from `snapshot_script`, replaced `__WSLTOP_CPU_COUNT__` with `0` as on first collection, and invoked `powershell.exe -NoProfile -NonInteractive -Command` twice from WSL. Parsed the returned JSON and checked `0 < available_bytes < total_bytes`.

| Sample | Elapsed | Total bytes | Available bytes | Observed processes |
| --- | --- | --- | --- | --- |
| 1 | 572 ms | 34,233,548,800 | 15,867,691,008 | 342 |
| 2 | 553 ms | 34,233,548,800 | 15,865,765,888 | 342 |

Both invocations exited successfully and returned non-null `host_memory`, a valid `system_times` CPU sample and 16 logical CPUs. This exercises PowerShell construction of `WsltopSystemTimes+MemoryStatus`, Marshal.SizeOf, `GlobalMemoryStatusEx` P/Invoke and serialization in the actual collector script. It is runtime evidence, not just a generated-script or arithmetic unit test. Earlier live TUI checks also displayed host physical RAM through the WSL executable.

This focused check does not establish equivalence with Task Manager or validate loaded Docker/WSLC scenarios. Native Windows wsltop execution/MSVC build on this host remains separate from this PowerShell ABI validation and the native Windows CI checks.
