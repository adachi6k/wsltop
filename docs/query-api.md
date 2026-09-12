# Internal read-only Query API

Next slice of [#24](https://github.com/adachi6k/wsltop/issues/24), tracked in
[#39](https://github.com/adachi6k/wsltop/issues/39), built on the
[snapshot store](snapshot-store.md) and [resource identities](resource-identity.md).
`QueryView::open(store, request)` selects one retained immutable snapshot. Its
four operations return the selected snapshot ID, capture timestamp, actual sample
window, and collection warnings alongside their data.

| Operation | Data |
| --- | --- |
| `get_system_summary` | Host CPU percentage, logical CPU count, physical memory information, independent environment observations |
| `list_resources` | Unique observed resources, sorted/filtered/limited |
| `inspect_resource(resource_id)` | One resource's numeric observations, command information, opaque ID, and observed parent IDs |
| `list_children(resource_id)` | Immediate observed children, with the same sorting/filtering options as listing |

The view never collects or refreshes. `Latest { max_age }` and explicit snapshot-ID
requests retain the store's freshness/consistency semantics. Unknown/expired
snapshots remain errors; unknown resource IDs are not interpreted as native PIDs.
Successful replies always identify their observation. The API is currently an
internal Rust interface, not a new serialized wire schema or CLI/MCP endpoint.

## Resource catalog and relationships

The catalog uses untruncated flat and PID sources plus the complete attribution
tree. Thus Windows application member PIDs and tree-only resources remain
inspectable even when absent from the current display. Identical observations
across projections share one ID. Conflicting observations for an ID, or duplicate
identities within a source list, fail with `AmbiguousResource` rather than selecting
one. Inconsistent non-finite observations can also fail this equality check.

Relationships come only from collected attribution groups, Docker/WSLC groups,
and Windows application membership. A resource can have multiple observed parents;
the API does not infer OS process ancestry by matching PIDs or guess across
namespaces. A known leaf returns an empty child list; an unknown parent is an error.
Cycles and self-parenting fail with `InvalidHierarchy`. Synthetic `unattributed`
display rows are not invented as resources.

Listing returns each catalog resource once, including parents and children; it
does not inject indented child rows or add their usage to parents. To explore a
particular parent, use `list_children`. This preserves group membership explicitly
while reusing `Sort::compare` from the existing CLI/TUI query layer. Both listing
operations filter before sorting and limiting, using optional environment, kind,
and case-sensitive name-substring filters. Default sorting is CPU descending and
the default limit is 30; a zero limit yields an empty list. Filters and limits
never alter the snapshot or its system summary.

CPU percentages retain the collected host-wide values. `cores_used` is derived
from that percentage and the snapshot's logical CPU count, or unavailable if
those inputs cannot produce a finite nonnegative value. Memory remains bytes;
no formatted strings replace numeric observations. Missing host/environment
observations remain missing. No uncollected resource limits or saturation values
are fabricated. Parent/child and cross-environment observations can overlap and
must not be summed into a host total.

## Remaining integration

The CLI does not yet call this module. A service adapter must own collection and
refresh/coalescing, allocate unique session epochs, detect namespace changes, and
coordinate in-flight samples before exposing external IDs. A later stdio MCP
adapter can map the four operations and typed errors to its protocol. The existing
CLI/JSON/TUI behavior is unchanged. The API contains no process/container actions.
