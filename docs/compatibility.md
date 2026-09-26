# Compatibility policy

wsltop 1.0 establishes the existing CLI, JSON output and read-only MCP tools as
public interfaces. The policy below applies to stable 1.x releases. Release
candidates allow these commitments to be checked before 1.0.0 is published.

## Versioning

- Patch releases fix bugs and improve correctness without intentionally breaking
  the documented interfaces.
- Minor releases may add optional functionality and response fields while
  preserving existing usage.
- Removing an interface, changing a field's type or meaning, or requiring a new
  argument for an existing operation requires a major release.

1.0 does not mean development has ended or that every proposed feature must be
implemented. Maintenance and focused additions can continue in 1.x. Force kill,
container stop and MCP mutation tools are not prerequisites for 1.0.

## Command line

Documented options, aliases and their meanings are part of the contract. One-shot
flat output remains the default, and `--once` remains accepted. Platform-specific
restrictions remain documented; for example, `--distro` selects the primary WSL
distribution when running the Windows executable.

Successful one-shot commands exit with status zero; failures exit nonzero.
Individual nonzero codes and diagnostic wording are not machine interfaces.
Use JSON for automation rather than parsing human-readable tables or diagnostics.
JSON output goes to stdout without diagnostic text mixed into it.

## JSON output

`--json` returns a top-level array of resource rows. `--tree --json` returns the
attribution tree object with `host_logical_cpu_count`, `groups`,
`unmapped_children`, `docker_groups`, `wslc_groups` and `windows_applications`.
Tree grouping represents observed attribution, not a universal process ancestry
tree. The existing row and group fields retain their types, units and meaning.

Resource rows retain `environment`, `kind`, `id`, `pid`, `name`, `cpu_percent`
and `memory_bytes`. Optional observations such as `source`, `args`, `ppid` and
`cpu_time_seconds` remain optional. Existing null and omitted-value meanings are
preserved; consumers must not treat missing observations as measured zero.
Process start identifiers used internally for safe actions are not JSON fields.

CPU percentages use the host-wide scale described in
[CPU accounting](cpu-accounting.md); visual CPU scaling does not change the
JSON units. Memory is expressed in bytes. Process memory and container memory
have different measurement scopes and must not be blindly summed.
Correcting collection or attribution bugs may change observed values in a patch
release without changing those definitions.

Consumers must ignore additional response fields and must not depend on field
order or whitespace. New optional fields can be added in minor releases.
Existing fields will not be removed, renamed, made mandatory when previously
optional, or assigned incompatible types or units in 1.x. New values in an
existing enum are treated as a breaking change unless introduced through a
separate opt-in interface.

## Read-only MCP

The four tools and their argument/result contracts documented in
[MCP usage](mcp.md) are public interfaces. Existing calls remain valid, including
their defaults and error-code meanings. Clients must send only documented
arguments and tolerate additional response fields. Protocol negotiation follows
the MCP versions supported by the server; dependency upgrades alone do not
authorize changes to wsltop's tool contracts.

MCP remains an explicitly started stdio service. Observation calls do not mutate
workloads. Any future mutation interface requires separate explicit opt-in; it
must not change the effect of an existing observation call.

Snapshot and resource IDs are opaque, scoped to their server observation, and
are not durable identifiers. Retention is bounded as documented. An unavailable
snapshot produces an error rather than silently querying a newer observation.

## Interactive UI and supported environments

The documented workflows and options are supported; exact layout, colors,
table spacing and diagnostic wording may evolve. Screenshots and terminal text
are not serialization formats.

Process termination is limited to the verified primary-WSL targets described in
[process actions](process-actions.md), requires explicit confirmation, and sends
normal termination only. 1.0 does not broaden that scope or remove identity
checks. Python and pidfd requirements apply to this optional action, not to
ordinary monitoring.

The supported runtime is Windows 11 with WSL2, using the Windows-native or
WSL-native executable. Docker and WSLC are optional. Missing optional collectors
can limit observations; 1.0 does not promise identical data on every host.
Published binaries target x64 Windows MSVC and x64 Linux GNU. Source builds use
current stable Rust; no older minimum Rust version is currently promised.

Rust modules, collector internals and internal Query APIs are implementation
details, not a stable Rust library API. wsltop is distributed as an application.
