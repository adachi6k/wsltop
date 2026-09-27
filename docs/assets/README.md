# README demo capture

`wsltop-demo.gif` is a recording of the real v0.5.0 TUI, captured on
2026-09-12 from release candidate `cb996f1`. It shows the compact CPU/RAM
summary, host history graphs, environment colors, additional WSL distribution
workloads, and running WSLC and Docker containers with process details.
All observations are live measurements.

The TUI ran in a dedicated 120-column, 34-row tmux session:

```sh
target/release/wsltop --interactive --color always --interval-ms 1000 \
  --limit 18 --container-process-limit 2
```

The demo containers, `wsltop-readme-wslc` and `wsltop-readme-docker`, used the
locally available `ghcr.io/adachi6k/docker-rv-dev-env:latest` image and ran
`/bin/sh -c 'yes > /dev/null'`. Each was limited to 0.25 CPU cores and 128 MiB
of memory. They were created only for this capture and removed afterward.
The separate `GitHubRunner` WSL workload was already running.

After the collectors and history graphs warmed up, ten ANSI screen captures
were recorded at two-second intervals as asciicast v2 and rendered with
`agg`, using the `github-dark` theme and a 16-pixel monospace font. Startup
frames are omitted; displayed values and rows are not edited. Only the flat
view is shown in this recording.

The summary uses host-wide CPU percentages; process and container rows use
the default core scale (one fully occupied core is 100%). Thus each demo
container appears near 25% in the resource list and near 1.6% of this
16-logical-CPU host in the summary.
