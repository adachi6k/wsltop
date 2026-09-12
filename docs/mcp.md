# Read-only MCP server

Tracked in [#43](https://github.com/adachi6k/wsltop/issues/43), within roadmap
[#24](https://github.com/adachi6k/wsltop/issues/24).

Requires wsltop v0.5.1 or later. Use a prebuilt binary or Cargo installation from
the [README](../README.md#quick-start); v0.5.0 does not include MCP.

Run `wsltop mcp` from your MCP client. The server uses local stdin/stdout, exposes
four read-only tools, and has no shell/terminate/kill/container-control tools or
network listener. Normal CLI/TUI and `--json` output remain separate.

For a client running in WSL, point its server configuration at your installed binary:

```json
{
  "mcpServers": {
    "wsltop": {
      "command": "/absolute/path/to/wsltop/target/release/wsltop",
      "args": ["mcp"]
    }
  }
}
```

For a Windows client, use the absolute path to the built `wsltop.exe` as `command`
with the same `args`. MCP client configuration formats vary; the example uses the
common `mcpServers` form. The executable requires the same WSL/interoperability
setup as regular wsltop. On Windows, normal primary-distro selection still applies
and may start that distro. Collection uses the local process's existing access.

Startup options are restricted to `--interval-ms N` (100–60000; default 3000),
`--wsl-only`, `--no-docker`, `--no-wslc`, and Windows-native `--distro NAME`.
`wsltop mcp --help` prints standalone help and exits. Display/JSON flags are not
accepted in server mode. Startup errors go to stderr; stdout is only MCP JSON-RPC
once the server starts. Closing stdin shuts it down.

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
summaries conservatively remain `null`. Incomplete environment samples also remain
unavailable. WSL-only native Linux sampling labels CPU scope `wsl_visible`; normal
and Windows-native sampling use `windows_host`. Host, environment, and parent/child
usage can overlap and must not be added together.

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
