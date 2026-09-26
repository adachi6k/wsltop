# TUI process termination

The first action is deliberately limited to Windows-native TUI → one ordinary
process in the collector's resolved primary WSL distribution. See the
[user controls](../README.md#terminating-a-selected-wsl-process).

## Target and execution boundary

The primary remote snapshot carries a boot ID, PID namespace, mount namespace
and effective user ID alongside the process sample. These are captured in the
same WSL invocation as the process list. Optional identity collection failure
does not disable monitoring; it disables the action. Linux-native and one-shot
observations do not expose this action scope.

The selected row carries its environment, source, kind, PID and start ticks;
it is never recovered by parsing rendered text. Confirmation pins the target
and is cancelled when its row is no longer visible or the collector scope
changes/becomes unavailable. Primary collector failure removes the usable
action scope even while the display retains earlier process values.

The worker executes the fixed embedded `src/terminate.py` using
`wsl.exe --distribution <resolved name> --exec python3 -I -c <fixed loader>`.
Arguments carry data, not shell fragments. No remote file or resident process
is installed. Python's isolated mode avoids importing code from the current
directory or user site. The operation uses the collected user identity;
it never asks WSL for root or attempts privilege escalation.

The helper compares its boot/namespaces/user with the observation, opens a
pidfd, then reads the target start ticks and namespaces. It signals the pidfd,
never the numeric PID. If the process exits between validation and signaling,
the handle cannot refer to a subsequently reused PID. Target startup before
pidfd acquisition is checked against the observed start identity. The existing
start-tick identity has the kernel's clock-tick resolution.

Python 3.9+ and kernel pidfd support are optional prerequisites for this action,
not for monitoring. The helper fails closed when unsupported, stale,
unverifiable or denied. Normal termination does not imply observed exit.

## Bounded execution

There is one in-flight worker and no queue. A ten-second absolute deadline is
checked immediately before sending the signal, and the Windows command wait
is bounded to twelve seconds. Host/guest clock disagreement can reject a valid
request; it must not weaken identity checks. Transport timeout or an absent
acknowledgment is an unknown outcome, not evidence that nothing happened. The
UI does not automatically retry. Exiting the UI after confirmation does not
revoke a request already dispatched.

## Validation

`cargo test --locked --all-targets` runs selection and confirmation regression
tests. On Unix it also runs `tests/terminate_backend.py` against the actual
embedded Python function, covering pidfd-before-validation, stale identity,
namespace/user mismatch, permissions, expiry and disposable live processes.

For an explicitly chosen live WSL distro, the Windows unit-test executable has
an ignored `action::tests::live_windows_to_wsl_termination` test. Set
`WSLTOP_ACTION_TEST_DISTRO`, then run that test with `--exact --ignored`. It
creates a disposable 30-second process, rejects changed identity/scope,
terminates the matching target and observes disappearance. Do not run it
against an unspecified/default target.

No force-kill, bulk/tree termination, container actions, generic shell API or
MCP action tools are included. Those remain separate decisions in #56–#58.
