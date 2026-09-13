# Manual MCP agent evaluation

Use this lightweight checklist to reassess agent usability after changing MCP tool
descriptions or APIs. It tests tool selection, arguments, and interpretation with
a real client; it complements the [stdio tests](mcp.md#verification-and-protocol-references).
See the [agent workflow example](mcp.md#agent-workflow-example) for the full flow.
This guide introduces no protocol, runtime, or server-side build heuristics.

## Codex registration and manual verification

Install wsltop v0.5.1 or later and use the executable appropriate for the client's
Windows or WSL environment, as described in the [quick start](mcp.md#quick-start).
Replace the placeholder with an absolute executable path:

```console
codex mcp add wsltop -- /absolute/path/to/wsltop mcp
codex mcp list
```

Expected: `wsltop` is registered with the intended Command and Args containing
`mcp`. Registration alone does not verify tool execution; enable/reload the server
in Codex and check discovery of the four wsltop tools. These commands follow the
[official Codex MCP configuration documentation](https://developers.openai.com/codex/mcp/).

Then ask:

```text
Use only wsltop MCP tools. Do not invoke wsltop from the shell.
Find the top 5 CPU-consuming resources.
```

Check the tool trace, not just the final answer: expect `list_resources` with CPU
descending order and `limit=5`. Without an explicit MCP request, Codex may choose
the shell CLI, as observed in the session behind this guide. This is client tool
selection behavior, not necessarily a wsltop server bug. The setup commands above
are separate from the agent's MCP-only diagnostic task.

## Evaluation conventions

Prefix each case with `Use only wsltop MCP tools. Do not invoke wsltop from the shell.`
Run cases separately, except case 7, which follows case 6. Tool patterns below are
pseudocode, not shell commands. `S` is the returned `snapshot.snapshot_id`; `R` is a
returned `resource_id` in that snapshot. `N` is an appropriate bounded listing limit.

- Start each new investigation with `max_age_ms=0` on a summary or resource listing.
  Use only `snapshot_id=S` for subsequent calls. Never combine both selectors.
- Preserve IDs verbatim: resource IDs are opaque and observation-scoped. After
  `snapshot_unavailable`, explicitly restart with a fresh sample and new resource
  IDs; never silently mix observations or invent an ID from a PID/name.
- Check CPU scope, core count, memory units, warnings, and unavailable values.
  Environment observations and parent/child values overlap and must not be summed.
  Duplicate workload observations are not independent consumers.
- Empty child/container listings are valid. `null` means unavailable, not zero;
  no observed container does not prove none exists. Missing coverage limits the
  conclusion, and warnings should be reflected when relevant.
- Judge the answer against returned data, not fixed process names or percentages.
  A single high reading is not evidence of unusual load without a baseline.

## Cases

### 1. What is using the most CPU right now?

- **Tools/arguments:** `list_resources(max_age_ms=0, sort_by="cpu", sort_order="desc", limit=1)`.
- **Pass:** Names the top observed resource and environment; reports CPU on the
  returned scale, or explains an empty result.
- **Failures:** Sorts ascending, uses stale conversation data, or labels host-wide
  CPU percentages as percentages of one core.

### 2. What is using the most memory right now?

- **Tools/arguments:** `list_resources(max_age_ms=0, sort_by="memory", sort_order="desc", limit=1)`.
- **Pass:** Names the largest observed memory consumer with environment and
  correctly converted units; acknowledges differing environment memory semantics.
- **Failures:** Uses CPU sorting, confuses bytes with GiB, or claims memory leaks
  or pressure solely from a large resource value.

### 3. Is the current load mainly from Windows or WSL?

- **Tools/arguments:** `get_system_summary(max_age_ms=0)`; optionally
  `list_resources(snapshot_id=S, environment="wsl", sort_by="cpu", sort_order="desc", limit=N)`
  or the corresponding `environment="windows"` call.
- **Pass:** Compares Windows and WSL CPU observations with host CPU context;
  explains overlap and missing coverage, including Docker within WSL when relevant.
- **Failures:** Adds environment totals, attributes all WSL activity to one distro,
  or treats missing totals as zero.

### 4. Is Docker responsible for the current CPU load?

- **Tools/arguments:** `get_system_summary(max_age_ms=0)`;
  `list_resources(snapshot_id=S, environment="docker", resource_kind="container", sort_by="cpu", sort_order="desc", limit=N)`;
  `list_children(snapshot_id=S, resource_id=R)` for a relevant container.
- **Pass:** Relates observed Docker usage to host load and identifies a container
  and children when available; accepts an empty listing without inventing a cause.
- **Failures:** Counts WSL plus Docker as separate additive load, double-counts
  container/child CPU, or claims Docker is idle because its summary is `null`.

### 5. Show me the top 5 CPU consumers.

- **Tools/arguments:** `list_resources(max_age_ms=0, sort_by="cpu", sort_order="desc", limit=5)`.
- **Pass:** Reports five ranked resources, or all available if fewer; includes
  environments, CPU values, and an overlap caveat where applicable.
- **Failures:** Returns a default-sized list, silently filters to one environment
  or only processes, or runs the shell CLI despite the MCP-only instruction.

### 6. Find the busiest resource and inspect it.

- **Tools/arguments:** `list_resources(max_age_ms=0, sort_by="cpu", sort_order="desc", limit=1)`;
  `inspect_resource(snapshot_id=S, resource_id=R)` using that result.
- **Pass:** Preserves the same snapshot and returned resource ID; inspection
  details support the answer without silently mixing in a new observation.
- **Failures:** Uses an old resource ID with a new snapshot, passes a PID as `R`,
  or invokes the shell CLI instead of MCP.

### 7. What is running underneath that resource?

- **Tools/arguments:** Following case 6, `list_children(snapshot_id=S, resource_id=R, sort_by="cpu", sort_order="desc", limit=N)`.
- **Pass:** Lists immediate observed children of the selected resource in its
  original snapshot; an empty list is valid. If expired, explicitly restarts
  selection and explains the changed observation.
- **Failures:** Guesses ancestry from PIDs, silently refreshes, calls an unrelated
  global listing, or treats no observed children as a tool failure.

### 8. Which WSL workload is using the most memory?

- **Tools/arguments:** `list_resources(max_age_ms=0, environment="wsl", sort_by="memory", sort_order="desc", limit=1)`;
  optionally `inspect_resource(snapshot_id=S, resource_id=R)`.
- **Pass:** Identifies the largest observed WSL memory consumer and its distro/source
  when returned; distinguishes missing source information from a known distro.
- **Failures:** Omits the WSL filter, selects a Windows VM aggregate instead,
  or invents a distro or container mapping.

### 9. Is any container causing unusual load?

- **Tools/arguments:** `get_system_summary(max_age_ms=0)`; two
  `list_resources(snapshot_id=S, environment=E, resource_kind="container", sort_by="cpu", sort_order="desc", limit=N)`
  calls with `E="docker"` and `E="wslc"`; inspect the winner with
  `inspect_resource(snapshot_id=S, resource_id=R)` and `list_children(snapshot_id=S, resource_id=R)`.
  Repeat listings with `sort_by="memory"` when memory pressure is relevant.
- **Pass:** Checks both backends, compares candidates within `S`, and explains
  the busiest container and observed children. Distinguishes high current usage
  from an anomaly requiring a baseline; empty results are acceptable.
- **Failures:** Checks Docker only, mixes snapshots between backends, invents
  abnormality from a single reading, or adds child usage to the container total.

### 10. My build is running slowly. Diagnose the likely cause.

- **Tools/arguments:** `get_system_summary(max_age_ms=0)`;
  `list_resources(snapshot_id=S, sort_by="cpu", sort_order="desc", limit=N)`;
  `inspect_resource(snapshot_id=S, resource_id=R)` and
  `list_children(snapshot_id=S, resource_id=R)` for the leading candidate;
  if needed, `list_resources(snapshot_id=S, resource_kind="container", sort_by="cpu", sort_order="desc", limit=N)`.
- **Pass:** Explains environment, container (or no observed container), and process
  using available evidence. Compares core consumption with host utilization and
  available memory; clearly labels the diagnosis as likely and states what is unknown.
- **Failures:** Declares disk I/O waits, a memory leak, exact build identity, or
  a definite cause without evidence; mistakes one busy core for a saturated host;
  invents a responsible container from prior conversation data.

## Observed Codex example

The README example summarizes a real Codex session, not a required outcome for
case 10. Its MCP-only investigation kept one snapshot for summary, CPU ranking,
resource inspection, immediate children, and a container-filtered listing.
WSL was the largest observed environment; Ubuntu's `gw_sh` used about one core.
Host CPU was about 25% across 16 logical CPUs, with about 5.7 GiB memory available.
No children or containers were observed. A concise, evidence-bounded answer was:

> The current load is mainly from WSL. The busiest process is using about one
> CPU core. No Docker or WSLC container is observed as responsible in this snapshot.
> Overall host CPU usage is moderate, so if that process is the build, limited
> parallelism is a likely bottleneck rather than total CPU saturation.

The sample does not establish the exact build task or disk I/O waits; those require
supporting observations elsewhere. Long opaque IDs and raw JSON-RPC/session logs
are deliberately omitted. This example does not claim all ten cases were run.

## Record and regression checks

Record the wsltop version/commit, client/model version, collector options, available
environments, case number, pass/fail/not-run, selected tools and argument patterns,
whether the snapshot was preserved, and a short reason. Use aliases such as `S`
and `R` in shared notes instead of opaque IDs or raw logs. Recheck failures after
tool description/API changes; distinguish client selection mistakes from server
errors and unavailable collector coverage.

Alongside manual checks, run the existing regression tests, including for docs-only
changes:

```console
cargo test --locked
cargo test --locked --test mcp_stdio
```

Passing these tests does not establish agent usability or real-host collector
coverage. Record ignored tests as ignored, not as passed manual scenarios.
