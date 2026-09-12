# v0.5.1 — Read-only MCP and distribution polish

wsltop v0.5.1 keeps the existing CLI/TUI experience and one-shot JSON compatible
with v0.5.0, and adds an optional read-only interface for MCP clients.

## Highlights

- `wsltop mcp` serves four tools over local stdio: `get_system_summary`,
  `list_resources`, `inspect_resource`, and `list_children`.
- Every successful response includes snapshot metadata. Preserve `snapshot_id`
  when inspecting or traversing resources from the same observation.
- Resource IDs are opaque and observation-scoped. A resource ID from one sample
  is not valid against a different snapshot, even if its PID is unchanged.
- Retained observations remain readable while a refresh runs. Collection failures
  preserve retained data without claiming it is a successful fresh sample.
- Process-detail failures preserve valid Docker/WSLC aggregate totals, with
  warnings still visible to the client.
- Cargo-binstall metadata follows the existing release archive layout;
  installation guidance favors prebuilt binaries and keeps source builds available.

See [MCP setup, options and snapshot semantics](mcp.md) for client configuration.
MCP uses the same native collection prerequisites as the CLI. It adds no network
listener, shell tools, process termination or container-control actions. Automatic
cross-observation identity continuity and live action revalidation remain future
work. The v0.5.0 TUI demo still represents this release's visual interface.

## Distribution

Archive naming and versioned inner directories remain unchanged:

- `wsltop-v0.5.1-x86_64-pc-windows-msvc.zip`
- `wsltop-v0.5.1-x86_64-unknown-linux-gnu.tar.gz`

Each archive contains its executable, README and LICENSE, with a `.sha256`
sidecar. Windows requires Windows 11 and a usable WSL2 distribution; the Linux
binary runs inside WSL2. Docker and WSLC remain optional.

After publication, install from [GitHub Releases](https://github.com/adachi6k/wsltop/releases/latest),
run `cargo binstall wsltop`, or build with `cargo install --locked wsltop`.

This is a release candidate document. Tagging and publication follow review and
validation of the candidate; this preparation does not publish v0.5.1.
