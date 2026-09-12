# Query service orchestration

Tracked in [#41](https://github.com/adachi6k/wsltop/issues/41), within roadmap
[#24](https://github.com/adachi6k/wsltop/issues/24).

The internal `QueryService` connects the existing `Monitor` collector to the
[snapshot store](snapshot-store.md) and [four read-only operations](query-api.md).
It owns both collector and cache and returns a query view of one observation.
The [stdio MCP adapter](mcp.md) exposes these operations in development builds.
There is no background sampler.

## Request policy

- A fresh `Latest { max_age }` request reuses the retained snapshot.
- An empty or stale latest result triggers one synchronous collection. Zero
  max-age explicitly forces collection, and returns that completed observation;
  it does not require an impossible zero elapsed time after collection.
- An explicit snapshot ID never collects, refreshes, or substitutes another ID.
  Unknown, expired, or invalidated IDs remain errors.
- Collection errors are returned directly, with no stale-success fallback.
  The previous snapshot remains available to explicit-ID reads while retained.
- Before insertion, the collected snapshot's combined query catalog is validated.
  Validation failure does not evict the previous observation or advance counters.
  Retention is checked again after validation, before committing the snapshot.

The service measures the actual collection call's start/completion and wall-clock
completion timestamp. Existing collection warnings and available summary fields
are preserved. This measures the overall collection window, not an assertion that
all environment collectors sampled at exactly the same instant.

## Session and resource lifetimes

Construction allocates a 256-bit service epoch from the OS random source through
`getrandom`. Failure to obtain randomness fails construction; there is no PID/time
fallback. Separate service instances do not intentionally share epochs or IDs.

Automatic host/distro/container namespace-continuity detection is not implemented
yet. Consequently **all resource IDs produced by this service are scoped to one
observation**, including processes with a known start ID. They remain resolvable
through their retained original snapshot, but cannot be used to claim the same
process across new collections. The lower-level store still supports
namespace-scoped IDs for a future producer with verified continuity.

`replace_collector` clears retained snapshots and rotates the namespace before
installing a replacement collector/configuration. The service owns its collector
and uses exclusive synchronous access, so replacement cannot race an in-flight
collection or an outstanding borrowed query view. The collector contract requires
its worker tasks to have finished before it returns. Existing `Monitor::sample`
joins its workers. Background refresh and async request coalescing are not added.

## Verification and next step

Tests count collection calls to verify reuse/refresh/pinned-read behavior, preserve
last-good state after collection/catalog failure, check observation/session ID
separation, and exercise configuration replacement. An explicit ignored-by-default
WSL smoke test uses the real `Monitor` with WSL-only collection, then lists and
inspects the retained data. It can be run with:

```console
cargo test --locked native_monitor_service_smoke -- --ignored
```

The shared service separates collector and retained-store locks. Pinned reads
build their response under the store lock without acquiring the collector lock;
native collection holds no store lock. Validation and insertion happen after
collection, under the store lock. The shared service has a fixed collector;
reconfiguration requires a new service/session.

The [MCP adapter](mcp.md) serializes collection-bearing requests, carries snapshot
metadata/errors, and documents observation-scoped IDs. Automatic namespace
continuity and any native action revalidation remain separate work.
