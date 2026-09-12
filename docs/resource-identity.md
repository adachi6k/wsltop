# Resource identity foundation

First implementation slice of [#24](https://github.com/adachi6k/wsltop/issues/24),
tracked in [#35](https://github.com/adachi6k/wsltop/issues/35).
This is an internal contract for the forthcoming read-only Query API, not a new
CLI/JSON/MCP interface. Existing `ResourceUsage` serialization is unchanged.

## Identity and lifetime

`ResourceIdentity` records a namespace scope, environment, resource kind, source,
native ID, and incarnation. `ResourceId` is an opaque string derived from that
structured identity. Pass it back intact; its encoding is private, versioned,
and not a security boundary. It is not a compact display label or a secret.

| Observation | ID lifetime | Process comparison |
| --- | --- | --- |
| Process/infra with PID and start ID | Same process generation within the same namespace scope | Compare scope, environment, kind, source, native ID, PID, and start ID |
| Process/infra missing PID or start ID | One observation | Unverifiable; never fall back to PID alone |
| Application, host, or container row | One observation | Unsupported by process comparison, even if a PID is present |

CPU, memory, command name, and sorting do not change an identity. `None` and an
explicit source remain distinct. Containers currently lack an incarnation field,
so the conservative contract does not preserve their IDs across observations.
Docker/WSLC child processes currently lack start IDs and also use observation
lifetimes. Synthetic `unattributed` rows are presentation values, not index entries.

## Caller responsibilities

`IdentityScope` and `ObservationId` are nonempty caller-supplied epochs, not values
inferred from a displayed PID. The future snapshot service must:

- Allocate a unique scope for its service/host/collector configuration and rotate
  it when a relevant host, distro, or container PID namespace restarts. In
  particular, an unnamed primary WSL source must not survive a primary-distro
  switch under the same scope. Distinct Docker daemon contexts need distinct
  scopes. A new service session must not reuse an old scope.
- Allocate a new observation ID for each immutable collected snapshot. Sorting
  or filtering that snapshot reuses its observation ID.
- Expire unavailable snapshots explicitly. A missing ID in an index means
  `UnknownResource`, not proof that the live resource has exited.

Automatic epoch allocation/restart detection and snapshot retention are **not
implemented in this slice**. Until that service exists, these types are an
internal foundation; they must not be exposed as persistent external handles.

## Query boundary and validation

`QuerySource::resource_index` builds a borrowed, read-only index of its untruncated
flat-view resources. A display limit cannot discard lookup candidates. An index
does not merge flat, PID, and tree views: tree traversal and application-member
inspection belong to the later Query API. IDs resolve only by exact lookup;
arbitrary caller strings are never decoded into trusted target coordinates.
Duplicate identities return `AmbiguousResource` instead of choosing a row.

`matches_process_observation` compares an identity with a separately supplied
process observation. Changed scope/PID/start identity is stale; missing generation
information is unverifiable. Application/host/container rows are rejected even
when they display a PID. This helper performs no collection and is not an action
authorization check. A future backend must acquire fresh native state and handle
the check/act race (for example, by retaining an appropriate native process
handle). No termination or container-control backend is added here.

The module and query entry point are intentionally allowed to be unused by the
production CLI until the Query API is wired in. Tests exercise identity lifetime,
PID reuse, namespace separation, aggregate rejection, ambiguous lookup, query
limits, and unchanged compatibility JSON.
