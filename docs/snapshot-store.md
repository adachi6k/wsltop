# Snapshot retention foundation

Second internal slice of [#24](https://github.com/adachi6k/wsltop/issues/24), tracked
in [#37](https://github.com/adachi6k/wsltop/issues/37), using
the [resource identity contract](resource-identity.md). There is no new CLI or
MCP entry point yet. Existing collection, TUI, and compatibility JSON are unchanged.

## Read contract

`SnapshotStore` owns completed `MonitorSnapshot` values and exposes only immutable
references. Each insertion receives a distinct opaque `SnapshotId`; reads never
sample or refresh anything. All subsequent queries for that ID see the same
resources, summary, warnings, capture timestamp, and sample window.

| Request | Result |
| --- | --- |
| `Latest { max_age }` | Latest retained observation if its age is at most `max_age`; otherwise `TooOld` |
| `Id(snapshot_id)` | That exact retained observation, even if a newer one exists |
| Unknown, evicted, expired, or invalidated ID | `SnapshotUnavailable`; never substitute the latest observation |
| Empty store or expired latest observation | `SnapshotUnavailable` |

Freshness and consistency are separate request forms. Clients that need consistency
reuse the returned ID; requesting latest repeatedly can return different observations.
A later service adapter can refresh on `TooOld`/empty-cache results, but a pinned
read must never refresh or fall back. Expired IDs are not retained as tombstones,
so unknown and expired IDs share the same error.

Retention expires at `age >= retention`; freshness accepts `age <= max_age`.
Age is measured by monotonic time from **collection completion**, not insertion
time. Wall-clock capture time is metadata only. The sample window uses the actual
monotonic collection start/end supplied by the producer, not the configured refresh
interval. Invalid/future/out-of-order completion times, missing untruncated query
sources, and already-expired samples are rejected without replacing the last good
observation. Warnings from a completed partial collection remain attached to it.

## Bounds and identity lifetime

Retention duration and capacity are explicit nonzero configuration. Insertion
removes expired entries and evicts the oldest observations until the count fits.
Reads reject expired observations even during idle periods; physical removal waits
for the next insertion or namespace rotation. Memory is bounded by snapshot count,
not by bytes per snapshot. Returned Rust borrows cannot outlive or mutate the store.

The caller supplies a unique service-session `IdentityScope`, never reused by
another store/session. The store allocates monotonically increasing observation
IDs and namespace epochs; counters fail instead of wrapping. Within an epoch,
known process generations keep resource IDs between snapshots. Weak/aggregate
identities receive the snapshot's observation ID and therefore change on insertion.

Before collecting after a namespace/configuration change, the owner calls
`rotate_namespace()`. This clears retained snapshots and changes the identity
scope, so even the same PID/start ID gets a different resource ID. Snapshot
sequence numbers do not reset. The owner must discard any in-flight collection
from the previous namespace and serialize rotation/collection/insertion; this
store does not supervise collector tasks or detect host/distro/container restarts.

## Integration still required

This module is intentionally unused by the production CLI until the Query service
is connected. The [internal Query API](query-api.md) supplies list/inspect/children/
summary operations on the retained source. The next service layer must supply
unique session epochs, actual collection timing, collector namespace-change
detection, and refresh/coalescing policy. Only then can the external snapshot
contract and MCP tools be exposed. Retaining an observation
does not make it suitable for destructive actions: live native revalidation is
still required.
