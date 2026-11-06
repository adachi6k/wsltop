# BuildKit review validation — 2026-11-07

This records the checks for [PR #82](https://github.com/adachi6k/wsltop/pull/82)
after the sampling fixes in `2c73897509d645aa81a98f29a5312639b416b8ac`.

## Automated checks

- `cargo fmt --all -- --check`
- `cargo test --locked --all-targets`: 258 unit tests and 2 integration tests
  passed; 4 live-host tests explicitly ignored.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`
- `RUSTFLAGS='-D warnings' cargo check --locked --all-targets --target x86_64-pc-windows-gnu`

The collector regression starts with no additional distro and discovers
`docker-desktop` on the next capture. One-shot completeness requires matching
source sets across the before, middle and after captures; streaming waits for
each newly discovered source's second snapshot.

The disabled-Docker regression covers primary and additional WSL sources, with
BuildKit cgroup metadata disappearing, appearing, or changing executor between
two snapshots. All six cases retain the process, its CPU delta and its current
RSS. The additional-stream regression also checks normalization before matching.

## Windows-native runtime

Built the Windows GNU release executable from the above commit in WSL and ran
the resulting `.exe` on Windows through WSL interoperability. Its SHA-256 was
`b4d26f8f609064df12af7f8ec3edf56df243b8df7cc324d148a6ec48ea297ae7`.
`--version` succeeded. The host reported 16 logical CPUs; default `Ubuntu` and
additional `docker-desktop` were already running. `docker.exe ps` was empty.

A controlled embedded BuildKit step used the locally cached
`rhysd/actionlint:1.7.7` image with this Dockerfile:

```dockerfile
FROM rhysd/actionlint:1.7.7
RUN sh -c 'while :; do :; done' & worker=$!; sleep 180; kill "$worker"; wait "$worker" || true
```

Launched with `docker.exe build --network=none --pull=false --no-cache
--progress=plain --output=type=cacheonly - < Dockerfile`. The step finished
successfully after 180 seconds. Its three process PIDs and shared
`0::/docker/buildkit/<executor>` membership were read directly from
`docker-desktop`'s `/proc` for comparison with the executable's output.

All observations used `--once --json --no-wslc --interval-ms 1000`:

| Additional options | Observed result |
| --- | --- |
| `--show-container-processes --limit 10000 --container-process-limit 10000` | One Docker BuildKit executor and all three member processes; no corresponding WSL rows. Executor CPU was 6.275% of the host and RSS was 2,596,864 bytes, equal to the process sum. |
| `--tree` | The same three PIDs were children of the Docker executor; none appeared in WSL groups or unmapped WSL children. |
| `--no-docker --limit 10000` | All three PIDs appeared as WSL processes from `docker-desktop`, with no Docker rows. Their CPU sum was 6.261% and RSS remained 2,596,864 bytes. |

The Docker-enabled capture emitted the expected unresolved-overlap warning;
the WSL kernel observation remains inclusive. CPU values above come from
different sampling windows and are not expected to be identical.

These are one-shot real-host checks. Distro startup during a sample, intermittent
cgroup metadata and stream baseline transitions are covered by automated
regressions; this run did not start/stop the user's distros or repeat native TUI
interaction or MSVC release packaging.
