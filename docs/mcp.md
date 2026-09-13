# Read-only MCP server

Tracked in [#43](https://github.com/adachi6k/wsltop/issues/43), within roadmap
[#24](https://github.com/adachi6k/wsltop/issues/24).

Requires wsltop v0.5.1 or later. Use a prebuilt binary or Cargo installation from
the [README](../README.md#quick-start); v0.5.0 does not include MCP.

Run `wsltop mcp` from your AI agent's MCP client. This is a **read-only,
observability-only** local stdio server: it exposes no process termination/kill,
shell or arbitrary command execution tools, container stop/control, or network
listener. Normal CLI/TUI and `--json` output remain separate.

## Quick start

Install wsltop v0.5.1 or later using the [README quick start](../README.md#quick-start).
Windows 11 with a usable WSL2 distribution is required; the Linux executable runs
inside WSL2, not on standalone Linux.
Configure your client to launch the executable with `["mcp"]` arguments and stdio
transport; the client starts the server, so no separate background service is needed.
Use an **absolute executable path**, not `~` or an unexpanded environment variable.
The examples use the common `mcpServers` form; adapt the surrounding configuration
to your client's documented stdio-server format.

### Client running inside WSL2

For a client running in WSL, point its server configuration at your installed binary.
For a default Cargo installation, replace `<user>` with your Linux username:

```json
{
  "mcpServers": {
    "wsltop": {
      "command": "/home/<user>/.cargo/bin/wsltop",
      "args": ["mcp"]
    }
  }
}
```

Check the installed executable's absolute path with `command -v wsltop` and use
that path if your Cargo install location differs.

### Client running on Windows

Use the absolute path to the installed or extracted `wsltop.exe`. JSON requires
escaped backslashes; replace this example path with your actual location:

```json
{
  "mcpServers": {
    "wsltop": {
      "command": "C:\\Tools\\wsltop\\wsltop.exe",
      "args": ["mcp"]
    }
  }
}
```

### Source build

Run `cargo build --release --locked` from the checkout. For a WSL client, replace
the command in the configuration above with your checkout's absolute build path:

```json
{
  "command": "/home/<user>/src/wsltop/target/release/wsltop",
  "args": ["mcp"]
}
```

For a Windows source build, use the absolute path to `target\release\wsltop.exe`.

Enable or reload the server in your client and check that it discovers the four
tools below. Start by asking “Why is my machine busy?”

The executable requires the same WSL/interoperability
setup as regular wsltop. On Windows, normal primary-distro selection still applies
and may start that distro. Collection uses the local process's existing access.

Startup options are restricted to `--interval-ms N` (100–60000; default 3000),
`--wsl-only`, `--no-docker`, `--no-wslc`, and Windows-native `--distro NAME`.
`wsltop mcp --help` prints standalone help and exits. Display/JSON flags are not
accepted in server mode. Once the server starts, stdout is exclusively MCP
JSON-RPC; startup/diagnostic output belongs on stderr. Do not use a launch wrapper
that prints banners to stdout. Closing stdin shuts it down.

<a id="agent-workflow"></a>

## Agent workflow example

User:

> My build is running slowly.
> Use wsltop MCP to identify the likely bottleneck and explain which environment,
> container, and process are responsible.

Suggested tool flow (shown as tool calls, not shell commands):

1. `get_system_summary(max_age_ms=0)` — collect fresh host CPU/RAM and environment observations;
   save the returned `snapshot.snapshot_id` as `S`.
2. `list_resources(snapshot_id=S, sort_by="cpu", sort_order="desc", limit=5)` —
   find busy resources; save a returned `resource_id` as `R`.
3. `inspect_resource(snapshot_id=S, resource_id=R)` — inspect that resource.
4. `list_children(snapshot_id=S, resource_id=R)` — drill into its immediate
   application/container/attribution children, if any.
5. If needed, `list_resources(snapshot_id=S, resource_kind="container", sort_by="cpu", sort_order="desc", limit=5)`
   — check observed containers. Add `environment="docker"` or `environment="wslc"`
   for a specific backend. To find the busiest Docker or WSLC container, compare
   both filtered listings using `S`; inspect the winner's children with its
   returned resource ID and the same `S`.

Use `sort_by="memory"` for memory diagnosis; filter by `environment` or
`resource_kind` when narrowing the investigation. Keep the returned `snapshot_id`
throughout a drill-down: `resource_id` is observation-scoped and must travel with
its snapshot ID. If that snapshot expires, start again and obtain new IDs.
Host, environment and parent/child usage overlap; **do not add them together**.

`S` and `R` above are placeholders for returned opaque IDs, not literal arguments
or IDs derived from a PID or name. Do not send `max_age_ms` alongside `snapshot_id`.
An empty child list is valid: it means no immediate children were observed.
An empty container list is also valid and is not a tool error. Say that no
containers were observed in this snapshot; a `null` environment total does not
necessarily mean zero usage or prove a backend is idle or absent. An empty
container list cannot always distinguish an unavailable backend from a successful
collection with no containers. Check snapshot warnings and available coverage.
Do not infer container membership unless the returned hierarchy supports it,
such as explicit `parent_ids` or a container's returned children. Process names
and source labels alone do not establish that a process runs inside Docker.

In the actual Codex session summarized in the [README](../README.md#agent-example-a-slow-build),
WSL had the largest environment CPU observation. Ubuntu's `gw_sh` used about
one core, no children or containers were observed, and total host CPU was about
25% across 16 logical CPUs with memory still available. If that process was the
build, limited parallelism was a likely explanation. The observations did not
establish its exact build command or disk I/O waits. This example illustrates
reasoning from one sample, not a server-side build heuristic or guaranteed diagnosis.

MCP CPU percentages use the collected host-wide scale; use `cores_used` to explain
per-resource core consumption and check `cpu_scope` for WSL-only observations.
Container totals already include their children, and WSL can include Docker work.
Independent sampling can also make child and container numbers differ slightly.
Avoid inferring a missing component's usage by subtracting overlapping totals.

Use the [manual agent evaluation guide](mcp-agent-evaluation.md) to recheck tool
selection and snapshot handling after changing descriptions or APIs.

### Memory and causal limits

High memory usage may indicate pressure, but wsltop does not by itself establish
paging, swapping, disk-I/O stalls, or memory-pressure causality. It does not
directly observe page fault rate, swap/pagefile I/O, disk I/O wait, memory stall /
PSI, or paging latency.

High memory usage alone does not establish a memory bottleneck. The presence of
the `Memory Compression` process does not prove paging or a paging bottleneck;
relatively low available memory does not prove swapping is occurring, and memory
usage does not establish disk I/O wait. Separate observed facts from hypotheses:
say "memory usage is high and may contribute," rather than "memory pressure is
the bottleneck" without additional supporting metrics. Use "likely" or "may
contribute" for hypotheses supported by observations, and identify what remains
unmeasured.

## Tools and snapshots

| Tool | Arguments |
| --- | --- |
| `get_system_summary` | Optional `snapshot_id` or `max_age_ms` |
| `list_resources` | Optional snapshot selection, filters, sorting, limit, and `parent` |
| `inspect_resource` | Required `snapshot_id` and `resource_id` |
| `list_children` | Required `snapshot_id` and `resource_id`; optional filters/sorting/limit |

Snapshot selection uses either a retained opaque `snapshot_id` or latest with
`max_age_ms` (default 3000). Do not supply both. Zero age forces a new collection.
The cache retains at most four snapshots for 60 seconds from collection completion;
capacity eviction can make a snapshot unavailable sooner. Exact-ID reads never
collect or fall back to a newer observation.

Listings accept `sort_by` (`cpu`, `memory`, `name`), `sort_order` (`asc`, `desc`),
`limit` (0–1000, default 30), `environment` (`windows`, `wsl`, `wslc`, `docker`),
`resource_kind` (`process`, `application`, `container`, `infra`, `host`), and
case-sensitive `name_contains`. Filters run before sorting/limiting. Supplying
`parent` to `list_resources` requires `snapshot_id` and selects immediate children,
equivalent to `list_children`. No process ancestry is guessed from bare PIDs.

All resource IDs are opaque and **observation-scoped**, including known-generation
processes. Preserve the returned snapshot ID when inspecting a resource or listing
children. After refresh, an old resource ID remains valid only within its retained
original snapshot. Automatic namespace continuity and destructive-action identity
revalidation remain future work.

## Results and errors

Successful tool results contain `structuredContent` and an equivalent JSON text
content block. Their payload is:

```json
{
  "snapshot": {
    "snapshot_id": "<opaque ID>",
    "captured_at_unix_ms": 1780000000000,
    "sample_window_ms": 3010,
    "warnings": [],
    "resource_id_scope": "observation",
    "cpu_scope": "windows_host"
  },
  "data": {}
}
```

`data` is a summary object, resource object, or array of resource objects. A resource
has `resource_id`, `parent_ids`, `cores_used`, and `usage`. `usage` preserves collected
numeric CPU percentages, memory bytes, optional CPU time, native IDs, and available
command/source fields. Native IDs are informational; pass the opaque resource ID
back to tools. Capture time is Unix milliseconds (nullable if the system clock
cannot be represented); the sample window describes the overall collection call.

Summary data includes host logical CPU count, host CPU percentage, host memory
(total/available/used bytes), and independent environment observations. Unavailable
values are `null`, not guessed zeros. Legacy optional collectors cannot distinguish
every successful-empty result from an unavailable backend, so empty Docker/WSLC
summaries conservatively remain `null`. WSL `cpu_percent` and `memory_bytes` are
independently nullable: missing kernel counters do not hide valid RSS, and an
incomplete process collection does not hide valid kernel CPU. The entire WSL
observation is `null` only when both metrics are unavailable.
WSL-only native Linux sampling labels CPU scope `wsl_visible`; normal
and Windows-native sampling use `windows_host`. Host, environment, and parent/child
usage can overlap and must not be added together.

WSL category CPU is the shared kernel total, sampled once through the primary
distribution from `/proc/stat`, including short-lived tasks and kernel work.
It can include other distributions even with `--wsl-only` and overlap container
statistics. WSL RAM remains observed process RSS; process rows keep their existing
sampling semantics. See [CPU accounting](cpu-accounting.md#wsl-category-cpu).

Unknown tools and invalid arguments produce JSON-RPC invalid-params errors before
collection. Operational failures use `isError: true` and an error object with a
stable `code`, such as `snapshot_unavailable`, `unknown_resource`,
`ambiguous_resource`, `invalid_hierarchy`, or `collection_failed`. Error messages
are diagnostic text, not a parsing contract. Collection/catalog failures preserve
last-good snapshots but do not return them as a successful fresh result.

Pinned snapshot reads bypass collection coordination and remain available while
a refresh runs; retention still applies at read time. Container process-detail
warnings do not invalidate successfully collected aggregate CPU/memory totals.
Collection-bearing requests are serialized. Concurrent latest requests can reuse
the first completed sample; forced-refresh requests each collect. Blocking native
collection runs off the async runtime. Cancellation does not forcibly terminate
an already-running native collection; its serialization permit stays held until
it finishes. Set client timeouts above the configured interval plus collector time.

## Verification and protocol references

The stdio tests exercise initialization, discovery, read-only annotations, argument
and operational errors, clean stdout, and EOF shutdown on Linux and Windows CI.
The WSL end-to-end test covers all four tools, concurrent latest requests, forced
refresh, and pinned resource inspection:

```console
cargo test --locked --test mcp_stdio
cargo test --locked --test mcp_stdio -- --ignored
```

Protocol lifecycle/framing comes from the [official Rust SDK](https://github.com/modelcontextprotocol/rust-sdk).
Interoperability is tested with MCP `2025-11-25`; version negotiation is handled by
the SDK. See the specification's [lifecycle](https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle),
[stdio transport](https://modelcontextprotocol.io/specification/2025-11-25/basic/transports),
and [tools](https://modelcontextprotocol.io/specification/2025-11-25/server/tools).
