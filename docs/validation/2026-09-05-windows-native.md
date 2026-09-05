# Windows-native release acceptance — 2026-09-05

This records executed checks for [issue #16](https://github.com/adachi6k/wsltop/issues/16).
It is a smoke-test record, not a claim that the entire real-host feature matrix
has been completed.

## Build and environment

- Packaging change: `d3b2441` in [PR #23](https://github.com/adachi6k/wsltop/pull/23),
  based on merged PR #22 (`3e9a5e7`). No Rust runtime changes in this PR.
- [Release dry-run CI](https://github.com/adachi6k/wsltop/actions/runs/33946332344):
  native Linux and Windows builds, archive transfer/checksums, and extracted
  executable checks passed. The GitHub release publication step was skipped.
- [PR CI](https://github.com/adachi6k/wsltop/actions/runs/33946332336):
  Linux tests/lint/build/package, Windows cross-check, and Windows native
  tests/release build/help/version checks passed.
- Host: Windows 11, OS build `10.0.26200.0`; Windows PowerShell `5.1.26100.9168`;
  Windows host CPU count reported by wsltop: 16 logical CPUs.
- WSL2: default `Ubuntu` and additional `docker-desktop`, both already running.
- Executables came from the CI archives, not an older local build. Windows used
  `x86_64-pc-windows-msvc`; WSL used `x86_64-unknown-linux-gnu`.

## Archive checks

Both downloads matched their SHA-256 files. Each archive contained the executable,
README, and LICENSE under its versioned directory. Extracted help/version checks
passed on the native CI runners; the extracted Windows executable also passed
help/version on this host.

| Archive | SHA-256 |
| --- | --- |
| `wsltop-v0.3.0-x86_64-pc-windows-msvc.zip` | `4c1bca4f893f354fa7991afa64b25f50dfbc4313b68b81a132f276edace7b3a0` |
| `wsltop-v0.3.0-x86_64-unknown-linux-gnu.tar.gz` | `ba9609d10fe3461262cff289d5f7dba3835ffcc7633ff4b0bd4fd159d78fd205` |

These are development artifacts named from `Cargo.toml`; no `v0.3.0` release or
tag was created by this verification. Actions artifact retention is finite.

## Windows-native CLI

| Scenario | Result |
| --- | --- |
| Explicit primary: `--distro Ubuntu --wsl-only --no-docker --once --json` | Exit 0; 30 primary process rows; primary `source` field omitted; Windows-normalization/disabled-attribution warning emitted |
| Lowercase primary: `--distro ubuntu --once --tree --json --no-docker --no-wslc --interval-ms 1000` | Exit 0; parsed tree JSON with 16 host logical CPUs and the documented group keys |
| Default selection: `--once --json --no-docker --no-wslc --interval-ms 1000` | Exit 0; Windows and WSL rows; additional `docker-desktop` source present |
| Nonexistent explicit distro with `--once --wsl-only --no-docker` | Exit 1 with a required remote `/proc` collection error |
| `--interactive --json` | Exit 1 with the explicit incompatibility error |

## Interactive runtime checks

Windows results below use a terminal-attached WSL interop launch of the packaged
Windows executable. This exercises Windows console APIs; it is not a WSL-native
substitute binary. A custom ConPTY capture helper produced unreliable screen
captures, so its rendering results are excluded. An independent interactive
Windows Terminal visual walkthrough remains unrecorded.

| Scenario | Result |
| --- | --- |
| Windows `--distro Ubuntu --interactive --wsl-only --no-docker --interval-ms 1000` | Flat header and updating WSL rows observed; `t` changed to tree; `i`, `h`, `0` accepted without exit; `q` exited 0; alternate-screen leave and cursor-show sequences observed |
| Windows `--interactive --no-docker --no-wslc --interval-ms 30000` | Flat/tree output included Windows, WSL, and `docker-desktop`; `Esc` exited 0 in approximately 170 ms measured around the input/wait operation; alternate-screen leave and cursor-show sequences observed |
| Windows nonexistent primary with `--interactive --wsl-only --no-docker --interval-ms 1000` | TUI remained responsive and `q` exited 0 with screen/cursor restoration. The narrow captured footer did not establish the complete error text, so error-message rendering is not marked verified |
| WSL `--interactive --no-docker --no-wslc --interval-ms 1000` | Flat and Windows/WSL rows observed; tree/toggle/scroll keys sent; `q` exited 0 in approximately 1 ms; all saved termios settings matched after exit; alternate screen and cursor restored |

Exit timings are individual smoke observations, not latency guarantees. The
automated cadence-wait regression provides the deterministic shutdown check.

## Remaining acceptance scenarios

- Starting/stopping/restarting a disposable additional distro during the TUI,
  including first-baseline loading and removal only after a confirmed stop.
  Only the active working Ubuntu and Docker Desktop distributions were available;
  neither was terminated for this test. Stateful discovery/baseline behavior is
  covered by unit tests, but this real-host lifecycle test remains pending.
- Deliberate transient optional-collector failure and recovery with last-good
  rows, plus a full-width visual error/footer and terminal-restoration walkthrough.
- Controlled one-core/four-core CPU comparison and optional Docker/WSLC runtime
  scenarios were not repeated in this session. This does not replace their
  release-acceptance requirements in the [test plan](../test-plan.md).

Issue #16 remains open until the outstanding acceptance scope is resolved.
