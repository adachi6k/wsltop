//! Read-only stdio MCP adapter. Protocol lifecycle/framing is owned by rmcp.

use crate::identity::IdentityError;
use crate::model::{EnvironmentKind, ResourceKind};
use crate::monitor::{Monitor, MonitorConfig};
use crate::query::{Sort, SortKey, SortOrder};
use crate::query_api::{ListOptions, QueryError, ResourceView, SnapshotMetadata};
use crate::query_service::{QueryService, ServiceError, SharedQueryService, SnapshotCollector};
use crate::snapshot_store::{SnapshotError, SnapshotRequest};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::error::Error;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

const TOOLS: [&str; 4] = [
    "get_system_summary",
    "list_resources",
    "inspect_resource",
    "list_children",
];
const DEFAULT_MAX_AGE_MS: u64 = 3000;
const MAX_LIMIT: usize = 1000;

pub fn run(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    if args == ["--help"] || args == ["-h"] {
        println!("wsltop mcp: read-only MCP over stdin/stdout\n\nCollector options: --interval-ms N (100..60000), --wsl-only, --no-docker,\n--no-wslc, --distro NAME (Windows-native only).\n\nTools: get_system_summary, list_resources, inspect_resource, list_children.\nCache: 4 snapshots, 60-second retention; latest max_age_ms defaults to 3000.\nResource IDs are observation-scoped; pass snapshot_id when inspecting resources.\nNo shell execution or process/container actions are exposed.");
        return Ok(());
    }
    let (config, distro) = collector_options(&args)?;
    let cpu_scope = if config.wsl_only && cfg!(unix) {
        "wsl_visible"
    } else {
        "windows_host"
    };
    let service = QueryService::new(
        Monitor::new(config, distro),
        NonZeroUsize::new(4).unwrap(),
        Duration::from_secs(60),
    )
    .map_err(|error| format!("MCP service initialization failed: {error:?}"))?;
    let server = McpServer::new(service, cpu_scope);
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(async {
            let running = server.serve(rmcp::transport::stdio()).await?;
            running.waiting().await?;
            Ok::<(), Box<dyn Error>>(())
        })
}

fn collector_options(args: &[String]) -> Result<(MonitorConfig, Option<String>), Box<dyn Error>> {
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--wsl-only" | "--no-docker" | "--no-wslc" => index += 1,
            "--interval-ms" | "--distro" => {
                if index + 1 == args.len() {
                    return Err(format!("{} requires a value", args[index]).into());
                }
                index += 2;
            }
            arg => return Err(format!("unsupported MCP startup argument: {arg}").into()),
        }
    }
    let options = crate::parse_args_from(args.iter().cloned())?;
    crate::validate_options(&options)?;
    if options.interval > Duration::from_secs(60) {
        return Err("MCP --interval-ms must be at most 60000".into());
    }
    Ok((
        MonitorConfig {
            sort: Sort::default(),
            interval: options.interval,
            limit: 30,
            show_wsl_host: true,
            wsl_only: options.wsl_only,
            no_wslc: options.no_wslc,
            no_docker: options.no_docker,
            hide_infra: false,
            show_container_processes: true,
            container_process_limit: 5,
            collect_windows_applications: true,
        },
        options.distro,
    ))
}

struct McpServer<C> {
    service: Arc<SharedQueryService<C>>,
    gate: Arc<tokio::sync::Semaphore>,
    cpu_scope: &'static str,
}

impl<C: SnapshotCollector> McpServer<C> {
    fn new(service: QueryService<C>, cpu_scope: &'static str) -> Self {
        Self {
            service: Arc::new(service.into_shared()),
            gate: Arc::new(tokio::sync::Semaphore::new(1)),
            cpu_scope,
        }
    }
}

impl<C: SnapshotCollector + Send + 'static> ServerHandler for McpServer<C> {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build());
        info.server_info = Implementation::new("wsltop", env!("CARGO_PKG_VERSION"));
        info.instructions = Some("Read-only local observations. Preserve snapshot_id across calls; resource IDs are opaque and observation-scoped. CPU percentages use the collected host-wide scale. Environment and parent/child usage can overlap; do not sum them. No actions or shell tools.".into());
        info
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if request.is_some_and(|request| request.cursor.is_some()) {
            return Err(invalid("tools/list does not use cursors"));
        }
        Ok(ListToolsResult {
            tools: TOOLS.iter().map(|&name| tool(name)).collect(),
            ..Default::default()
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        TOOLS
            .contains(&name)
            .then(|| tool(TOOLS.iter().copied().find(|tool| *tool == name).unwrap()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let args = Arguments::parse(&request.name, request.arguments.unwrap_or_default())?;
        let name = request.name.into_owned();
        let service = self.service.clone();
        let cpu_scope = self.cpu_scope;
        let permit = if args.snapshot_id.is_none() {
            Some(
                self.gate
                    .clone()
                    .acquire_owned()
                    .await
                    .map_err(|_| ErrorData::internal_error("service closed", None))?,
            )
        } else {
            None
        };
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit; // Held even if the request is cancelled mid-collection.
            execute(&service, &name, args, cpu_scope)
        })
        .await
        .map_err(|_| ErrorData::internal_error("collection worker failed", None))?;
        Ok(result.into())
    }
}

fn invalid(message: impl Into<String>) -> ErrorData {
    ErrorData::invalid_params(message.into(), None)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Arguments {
    snapshot_id: Option<String>,
    max_age_ms: Option<u64>,
    resource_id: Option<String>,
    parent: Option<String>,
    sort_by: Option<String>,
    sort_order: Option<String>,
    limit: Option<usize>,
    environment: Option<String>,
    resource_kind: Option<String>,
    name_contains: Option<String>,
}

impl Arguments {
    fn parse(name: &str, raw: Map<String, Value>) -> Result<Self, ErrorData> {
        if !TOOLS.contains(&name) {
            return Err(invalid(format!("unknown tool: {name}")));
        }
        let listing = matches!(name, "list_resources" | "list_children");
        for (key, value) in &raw {
            let allowed = key == "snapshot_id"
                || (key == "max_age_ms" && matches!(name, "get_system_summary" | "list_resources"))
                || (key == "resource_id" && matches!(name, "inspect_resource" | "list_children"))
                || (key == "parent" && name == "list_resources")
                || (listing
                    && [
                        "sort_by",
                        "sort_order",
                        "limit",
                        "environment",
                        "resource_kind",
                        "name_contains",
                    ]
                    .contains(&key.as_str()));
            if !allowed || value.is_null() {
                return Err(invalid(format!("invalid argument: {key}")));
            }
        }
        let args: Self = serde_json::from_value(Value::Object(raw))
            .map_err(|error| invalid(error.to_string()))?;
        if args.snapshot_id.is_some() && args.max_age_ms.is_some() {
            return Err(invalid("snapshot_id and max_age_ms are mutually exclusive"));
        }
        if matches!(name, "inspect_resource" | "list_children")
            && (args.snapshot_id.is_none() || args.resource_id.is_none())
        {
            return Err(invalid("snapshot_id and resource_id are required"));
        }
        if args.parent.is_some() && args.snapshot_id.is_none() {
            return Err(invalid("parent requires snapshot_id"));
        }
        for id in [&args.snapshot_id, &args.resource_id, &args.parent]
            .into_iter()
            .flatten()
        {
            if id.is_empty() || id.chars().count() > 16384 {
                return Err(invalid("IDs must contain 1..16384 characters"));
            }
        }
        if args.limit.is_some_and(|limit| limit > MAX_LIMIT) {
            return Err(invalid("limit must be 0..1000"));
        }
        if args
            .name_contains
            .as_ref()
            .is_some_and(|name| name.chars().count() > 1024)
        {
            return Err(invalid("name_contains must be at most 1024 characters"));
        }
        args.options()?;
        Ok(args)
    }

    fn options(&self) -> Result<ListOptions<'_>, ErrorData> {
        Ok(ListOptions {
            sort: Sort {
                key: self
                    .sort_by
                    .as_deref()
                    .map(SortKey::parse)
                    .transpose()
                    .map_err(invalid)?
                    .unwrap_or_default(),
                order: self
                    .sort_order
                    .as_deref()
                    .map(SortOrder::parse)
                    .transpose()
                    .map_err(invalid)?
                    .unwrap_or_default(),
            },
            limit: self.limit.unwrap_or(30),
            name_contains: self.name_contains.as_deref(),
            environment: self
                .environment
                .as_deref()
                .map(|env| match env {
                    "windows" => Ok(EnvironmentKind::Windows),
                    "wsl" => Ok(EnvironmentKind::Wsl),
                    "wslc" => Ok(EnvironmentKind::WslContainer),
                    "docker" => Ok(EnvironmentKind::Docker),
                    _ => Err(invalid("invalid environment")),
                })
                .transpose()?,
            resource_kind: self
                .resource_kind
                .as_deref()
                .map(|kind| match kind {
                    "process" => Ok(ResourceKind::Process),
                    "application" => Ok(ResourceKind::Application),
                    "container" => Ok(ResourceKind::Container),
                    "infra" => Ok(ResourceKind::Infra),
                    "host" => Ok(ResourceKind::Host),
                    _ => Err(invalid("invalid resource_kind")),
                })
                .transpose()?,
        })
    }
}

fn tool(name: &'static str) -> Tool {
    let listing = matches!(name, "list_resources" | "list_children");
    let detail = matches!(name, "inspect_resource" | "list_children");
    let mut properties = Map::new();
    properties.insert("snapshot_id".into(), json!({"type":"string","minLength":1,"maxLength":16384,"description":"Opaque retained snapshot ID; never parse it."}));
    if !detail {
        properties.insert("max_age_ms".into(), json!({"type":"integer","minimum":0,"description":"Latest snapshot age bound; default 3000, zero forces collection. Mutually exclusive with snapshot_id."}));
    }
    if detail {
        properties.insert(
            "resource_id".into(),
            json!({"type":"string","minLength":1,"maxLength":16384}),
        );
    }
    if name == "list_resources" {
        properties.insert("parent".into(), json!({"type":"string","minLength":1,"maxLength":16384,"description":"Immediate children of this resource; requires snapshot_id."}));
    }
    if listing {
        for (key, schema) in [
            (
                "sort_by",
                json!({"type":"string","enum":["cpu","memory","name"]}),
            ),
            ("sort_order", json!({"type":"string","enum":["asc","desc"]})),
            (
                "limit",
                json!({"type":"integer","minimum":0,"maximum":1000,"default":30}),
            ),
            (
                "environment",
                json!({"type":"string","enum":["windows","wsl","wslc","docker"]}),
            ),
            (
                "resource_kind",
                json!({"type":"string","enum":["process","application","container","infra","host"]}),
            ),
            (
                "name_contains",
                json!({"type":"string","maxLength":1024,"description":"Case-sensitive name substring."}),
            ),
        ] {
            properties.insert(key.into(), schema);
        }
    }
    let required = if detail {
        vec!["snapshot_id", "resource_id"]
    } else {
        vec![]
    };
    let schema = json!({"type":"object","properties":properties,"required":required,"additionalProperties":false});
    let description = match name {
        "get_system_summary" => "Get numeric system observations and snapshot metadata. Environment totals may overlap.",
        "list_resources" => "List unique observed resources with filters and shared sorting. Pass snapshot_id to preserve observation consistency.",
        "inspect_resource" => "Inspect one opaque resource ID in its original retained snapshot.",
        _ => "List immediate observed attribution/application children within a retained snapshot. Child usage is included in parents.",
    };
    Tool::new(name, description, schema.as_object().unwrap().clone()).with_annotations(
        ToolAnnotations::from_raw(None, Some(true), Some(false), Some(false), Some(false)),
    )
}

fn metadata(meta: SnapshotMetadata<'_>) -> Value {
    json!({"snapshot_id": meta.snapshot_id, "captured_at_unix_ms": meta.captured_at.duration_since(UNIX_EPOCH).ok().and_then(|d| u64::try_from(d.as_millis()).ok()),
        "sample_window_ms":u64::try_from(meta.sample_window.as_millis()).ok(), "warnings":meta.warnings, "resource_id_scope":"observation"})
}

fn resource(view: ResourceView<'_>) -> Value {
    json!({"resource_id":view.resource_id,"parent_ids":view.parent_ids,"cores_used":view.cores_used,"usage":view.resource})
}

fn execute<C: SnapshotCollector>(
    service: &SharedQueryService<C>,
    name: &str,
    args: Arguments,
    cpu_scope: &str,
) -> CallToolResult {
    let request = args
        .snapshot_id
        .as_deref()
        .map(SnapshotRequest::Id)
        .unwrap_or(SnapshotRequest::Latest {
            max_age: Duration::from_millis(args.max_age_ms.unwrap_or(DEFAULT_MAX_AGE_MS)),
        });
    service.query(request, |view| {
    let options = args
        .options()
        .expect("arguments validated before collection");
    let (meta, data) = match name {
        "get_system_summary" => {
            let reply = view.get_system_summary();
            let environments: Map<String, Value> = ["windows", "wsl", "wslc", "docker"].into_iter().zip(&reply.data.environments.0)
                .map(|(name, usage)| (name.into(), usage.map_or(Value::Null, |usage| json!({"cpu_percent":usage.cpu_percent,"memory_bytes":usage.memory_bytes})))).collect();
            (
                metadata(reply.snapshot),
                json!({"host_logical_cpu_count":reply.data.host_logical_cpu_count,"host_cpu_percent":reply.data.host_cpu_percent,
                "host_memory":reply.data.host_memory.map(|m| json!({"total_bytes":m.total_bytes,"available_bytes":m.available_bytes,"used_bytes":m.used_bytes()})),"environments":environments}),
            )
        }
        "inspect_resource" => match view.inspect_resource(args.resource_id.as_deref().unwrap()) {
            Ok(reply) => (metadata(reply.snapshot), resource(reply.data)),
            Err(error) => return tool_error(ServiceError::Query(error)),
        },
        _ => {
            let reply = if name == "list_children" || args.parent.is_some() {
                match view.list_children(
                    args.resource_id
                        .as_deref()
                        .or(args.parent.as_deref())
                        .unwrap(),
                    &options,
                ) {
                    Ok(reply) => reply,
                    Err(error) => return tool_error(ServiceError::Query(error)),
                }
            } else {
                view.list_resources(&options)
            };
            (
                metadata(reply.snapshot),
                Value::Array(reply.data.into_iter().map(resource).collect()),
            )
        }
    };
    let mut meta = meta;
    meta["cpu_scope"] = json!(cpu_scope);
    CallToolResult::structured(json!({"snapshot":meta,"data":data}))
    }).unwrap_or_else(tool_error)
}

fn tool_error(error: ServiceError) -> CallToolResult {
    let code = match &error {
        ServiceError::Snapshot(error) | ServiceError::Query(QueryError::Snapshot(error)) => {
            match error {
                SnapshotError::SnapshotUnavailable => "snapshot_unavailable",
                SnapshotError::TooOld => "snapshot_too_old",
                SnapshotError::MissingQuerySource => "missing_query_source",
                _ => "snapshot_error",
            }
        }
        ServiceError::Query(QueryError::Identity(error)) => match error {
            IdentityError::UnknownResource => "unknown_resource",
            IdentityError::AmbiguousResource => "ambiguous_resource",
            _ => "identity_error",
        },
        ServiceError::Query(QueryError::InvalidHierarchy) => "invalid_hierarchy",
        ServiceError::Collection(_) => "collection_failed",
        ServiceError::Entropy(_) | ServiceError::LockPoisoned => "service_error",
    };
    CallToolResult::structured_error(json!({"error":{"code":code,"message":format!("{error:?}")}}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argument_validation_and_startup_options_are_narrow() {
        let names = [
            "get_system_summary",
            "list_resources",
            "inspect_resource",
            "list_children",
        ];
        for name in names {
            assert_eq!(tool(name).name, name);
        }
        let unicode = "名".repeat(1024);
        assert!(Arguments::parse(
            "list_resources",
            json!({"name_contains":unicode})
                .as_object()
                .unwrap()
                .clone()
        )
        .is_ok());
        for (name, args) in [
            ("get_system_summary", json!({"limit":1})),
            (
                "inspect_resource",
                json!({"snapshot_id":"s","resource_id":"r","max_age_ms":0}),
            ),
            ("list_resources", json!({"sort_order":"sideways"})),
            ("list_resources", json!({"resource_kind":"residual"})),
            ("list_resources", json!({"name_contains":"名".repeat(1025)})),
            ("get_system_summary", json!({"snapshot_id":""})),
        ] {
            assert!(Arguments::parse(name, args.as_object().unwrap().clone()).is_err());
        }
        assert!(collector_options(&["--once".into()]).is_err());
        assert!(collector_options(&["--interval-ms".into()]).is_err());
        let (config, _) = collector_options(&[
            "--wsl-only".into(),
            "--no-docker".into(),
            "--interval-ms".into(),
            "100".into(),
        ])
        .unwrap();
        assert!(
            config.wsl_only
                && config.no_docker
                && config.collect_windows_applications
                && config.show_container_processes
        );
    }

    #[test]
    fn operational_errors_use_structured_tool_results() {
        for (error, code) in [
            (
                ServiceError::Collection("offline".into()),
                "collection_failed",
            ),
            (
                ServiceError::Snapshot(SnapshotError::SnapshotUnavailable),
                "snapshot_unavailable",
            ),
            (
                ServiceError::Query(QueryError::Identity(IdentityError::AmbiguousResource)),
                "ambiguous_resource",
            ),
            (
                ServiceError::Query(QueryError::InvalidHierarchy),
                "invalid_hierarchy",
            ),
        ] {
            let result = tool_error(error);
            assert_eq!(result.is_error, Some(true));
            assert_eq!(result.structured_content.unwrap()["error"]["code"], code);
        }
    }
}
