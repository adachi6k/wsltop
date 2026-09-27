# Primary WSL process termination — 2026-09-26

Scope: #55, Windows-native TUI → the already resolved primary WSL distribution.
The user accepted Python 3.9+ and kernel pidfd as optional prerequisites for
termination. Monitoring does not acquire those dependencies.

## Checks

- Linux `cargo test --locked --all-targets`: 230 unit tests and 2 MCP integration
  tests passed; 4 existing live-environment tests ignored.
- The Rust suite invokes the actual embedded Python helper's 10 regression
  tests: no numeric-PID signaling, acquisition before validation, stale start
  identity, boot/namespaces/user mismatch, target exit, permission failure,
  unsupported kernel, expiry, and live disposable processes. A process that
  ignores SIGTERM remained alive after an accepted signal, as expected.
- TUI regressions cover sorting, filtering, PID reuse, cancellation, target
  disappearance/scope changes, undersized confirmation display, and responsive
  input with one in-flight request.
- Linux Clippy with `-D warnings` and Windows GNU target check with
  `RUSTFLAGS='-D warnings'`: passed.
- Windows GNU unit-test executable: passed before the final small-screen test
  was added; the final change is also covered by GitHub Windows CI.
- Windows console integration test: normal `q`, classic-mode `Esc`, and startup
  error restored the original console modes.

## Live Windows → Ubuntu check

Cross-compiled the native Windows test executable with the existing temporary
MinGW toolchain and invoked:

```
WSLTOP_ACTION_TEST_DISTRO=Ubuntu
action::tests::live_windows_to_wsl_termination --exact --ignored --nocapture
```

The test creates only its own 30-second Python child. It obtains the target
identity through the real `multiwsl::snapshot` collector, exercises the real
Windows command executor and embedded Python transport, and checks:

1. Changed start identity is rejected without signaling.
2. Changed mount namespace is rejected without signaling.
3. Matching identity receives SIGTERM.
4. The matching process disappears from subsequent observations.

Result: passed (0.84 s). Windows/WSL interop needed execution outside the command
sandbox; no target distro configuration or installed packages were changed.

The manual live test validates the backend path. Confirmation/selection are
covered with synthetic TUI input and TestBackend rendering; no claim is made
that a human interacted with the screen during this run.

## Limits

Only primary WSL ordinary processes are supported. Unknown identity, init,
foreign/nested namespaces, aggregate/container/Windows rows are rejected.
Accepted is not exited; transport timeout is an unknown outcome. No kill
fallback, privilege escalation, installer, queue, force-kill, or MCP mutation
was added. Python/kernel absence is handled as unsupported; the live test host
itself has those capabilities.
