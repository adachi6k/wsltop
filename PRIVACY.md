# wsltop Privacy Policy

Effective date: September 27, 2026.

This policy describes wsltop 1.0.0, published by Adachi6k at
https://github.com/adachi6k/wsltop. It covers the Windows and WSL executables,
terminal interface, JSON output, and optional MCP server.

## Information accessed and purpose

wsltop reads system and workload information to display resource usage and
explain which applications, processes, distributions, and containers contribute
to it. Depending on the enabled collectors and available permissions, this
includes CPU and memory counters, process IDs and parent relationships,
executable names, command lines and arguments, process start times, application
metadata, WSL distribution names, container names and IDs, and kernel/cgroup
identifiers. Process identity checks for the optional termination action also
use user IDs and namespace identifiers.

Names, paths, arguments, and diagnostic messages can contain personal or
confidential information, including usernames, project names, document paths,
or secrets placed in command arguments. wsltop does not guarantee redaction of
these values. It reads operating-system and container metadata; it does not
scan document contents as part of resource monitoring.

## Processing, output, and sharing

wsltop processes observations in the running program and presents them in the
terminal or through its requested text/JSON output. It has no built-in analytics,
advertising, telemetry, crash-report upload, or automatic update checker, and
does not send observations to the wsltop maintainer.

When you configure a client to run `wsltop mcp`, the client can request workload
observations through local standard input/output. The server does not open a
network listener. The client may record responses, include them in conversations,
or send them to an AI service according to its own configuration and privacy
policy. Local stdio transport does not mean that the client keeps data local.
Review the client's permissions, retention, and provider settings before enabling
this integration. Remove or disable the integration to stop future access.
The MCP interface exposes observations only, not process termination or arbitrary
command execution tools.

## External tools and network connections

Collection invokes installed Windows/WSL tools, PowerShell, Docker, and/or WSLC
as appropriate. Docker uses your configured CLI context and connection settings;
these may address a remote daemon. Requests to such a daemon and its responses
can therefore cross the network. Authentication, transport security, and logs
maintained by those tools or services are governed by their configuration and
policies. wsltop does not provide a separate cloud account or credential store.

Downloading or installing wsltop through GitHub, crates.io, or WinGet contacts
those distribution services under their respective policies. This is separate
from wsltop's runtime monitoring.

## Retention and deletion

wsltop does not maintain a persistent observation database or write monitoring
history to a log file of its own. Sampling baselines, display history, cached
metadata, and snapshots are held in process memory while running. The MCP
snapshot cache is configured for up to four snapshots with a 60-second retention
window; this limits snapshot availability, not secure erasure of memory. Other
live collector state may remain until replaced or the process exits.

Exiting wsltop releases its process memory. The operating system may separately
retain terminal scrollback, swap/pagefile contents, or diagnostic dumps. Output
you redirect to a file, capture in screenshots, or share with a client is outside
wsltop's retention control. Delete those copies through the program or service
that stores them; stopping wsltop does not delete previously exported data.

## Your choices

Run wsltop only where you are authorized to inspect workloads. You can disable
Docker or WSLC collection with `--no-docker` or `--no-wslc`. Display filters are
not a guarantee that underlying information was never collected. Avoid placing
secrets in command-line arguments and inspect output before sharing it.

The Windows terminal interface can, after explicit confirmation, request normal
termination of an eligible process in the primary WSL distribution. Identity
checks and action results support that operation; it does not upload information
to the maintainer. MCP remains read-only.

## Contact and policy changes

For non-sensitive privacy questions, contact the maintainer through
https://github.com/adachi6k/wsltop/issues. Do not post private process data,
credentials, or confidential logs in public issues. For security-sensitive
reports, follow https://github.com/adachi6k/wsltop/blob/main/SECURITY.md.

Changes to data handling will be reflected in this policy with an updated
effective date. This policy does not replace the policies of your MCP client,
external tools, operating system, or distribution services.
