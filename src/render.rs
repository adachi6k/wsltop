use crate::attribution::{AttributionTree, MappingStatus};
use crate::model::{EnvironmentKind, ResourceUsage};
use crate::monitor::MonitorSnapshot;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CpuScale {
    #[default]
    Core,
    Host,
}

impl CpuScale {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "core" => Ok(Self::Core),
            "host" => Ok(Self::Host),
            _ => Err(format!(
                "invalid CPU scale {value:?}; expected core or host"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Core => "1 core = 100%",
            Self::Host => "whole host = 100%",
        }
    }

    fn value(self, host_wide: f64, cpus: u32) -> f64 {
        match self {
            Self::Core => host_wide * cpus as f64,
            Self::Host => host_wide,
        }
    }
}

pub fn flat(snapshot: &MonitorSnapshot, scale: CpuScale) -> String {
    let mut out = String::new();
    let show_container_processes = snapshot.resources.iter().any(|row| {
        matches!(
            row.environment,
            EnvironmentKind::Docker | EnvironmentKind::WslContainer
        ) && row.kind == crate::model::ResourceKind::Process
            && row.source.is_some()
    });
    let _ = writeln!(
        out,
        "Host logical CPUs: {} | CPU scale: {}",
        snapshot.host_logical_cpu_count,
        scale.label()
    );
    let _ = writeln!(
        out,
        "{:<7} {:<11} {:>7} {:>9} {:>12} COMMAND",
        "ENV", "TYPE", "CPU%", "MEM", "ID/PID"
    );
    let _ = writeln!(out, "{}", "-".repeat(86));
    for row in &snapshot.resources {
        if matches!(
            row.environment,
            EnvironmentKind::Docker | EnvironmentKind::WslContainer
        ) && row.kind == crate::model::ResourceKind::Process
            && row.source.is_some()
        {
            continue;
        }
        let _ = writeln!(
            out,
            "{:<7} {:<11} {:>6.2}% {:>9} {:>12}  {}",
            env_name(row.environment),
            row.kind.as_str(),
            scale.value(row.cpu_percent, snapshot.host_logical_cpu_count),
            format_bytes(row.memory_bytes),
            display_id(row),
            display_name(row)
        );
        if matches!(
            row.environment,
            EnvironmentKind::Docker | EnvironmentKind::WslContainer
        ) && row.kind == crate::model::ResourceKind::Container
            && show_container_processes
        {
            let groups = if row.environment == EnvironmentKind::Docker {
                &snapshot.tree.docker_groups
            } else {
                &snapshot.tree.wslc_groups
            };
            if let Some(group) = groups.iter().find(|group| group.container.id == row.id) {
                let displayed: Vec<_> = snapshot
                    .resources
                    .iter()
                    .filter(|process| {
                        process.environment == row.environment
                            && process.kind == crate::model::ResourceKind::Process
                            && process.source.as_deref() == Some(row.id.as_str())
                    })
                    .collect();
                for process in &displayed {
                    let _ = writeln!(
                        out,
                        "{:<7} {:<11} {:>6.2}% {:>9} {:>12}    |- {}",
                        "",
                        "process",
                        scale.value(process.cpu_percent, snapshot.host_logical_cpu_count),
                        format_bytes(process.memory_bytes),
                        display_id(process),
                        process.name
                    );
                }
                if group.children.len() > displayed.len() {
                    let displayed_ids: std::collections::HashSet<_> = displayed
                        .iter()
                        .map(|process| process.id.as_str())
                        .collect();
                    let omitted: Vec<_> = group
                        .children
                        .iter()
                        .filter(|process| !displayed_ids.contains(process.id.as_str()))
                        .collect();
                    let omitted_cpu: f64 = omitted.iter().map(|process| process.cpu_percent).sum();
                    let _ = writeln!(
                        out,
                        "{:<7} {:<11} {:>6.2}% {:>9} {:>12}    |- {} more processes",
                        "",
                        "processes",
                        scale.value(omitted_cpu, snapshot.host_logical_cpu_count),
                        "-",
                        "-",
                        omitted.len()
                    );
                }
                let _ = writeln!(
                    out,
                    "{:<7} {:<11} {:>6.2}% {:>9} {:>12}    `- unattributed",
                    "",
                    "residual",
                    scale.value(
                        group.unattributed_cpu_percent,
                        snapshot.host_logical_cpu_count
                    ),
                    "-",
                    "-"
                );
                if group.over_attributed_cpu_percent > 0.0 {
                    let _ = writeln!(
                        out,
                        "{:<7} {:<11} {:>6.2}% {:>9} {:>12}    `- over-attributed",
                        "",
                        "residual",
                        scale.value(
                            group.over_attributed_cpu_percent,
                            snapshot.host_logical_cpu_count
                        ),
                        "-",
                        "-"
                    );
                }
            }
        }
    }
    out
}

pub fn tree(snapshot: &MonitorSnapshot, scale: CpuScale) -> String {
    tree_model(&snapshot.tree, snapshot.host_logical_cpu_count, scale)
}

fn tree_model(tree: &AttributionTree, cpus: u32, scale: CpuScale) -> String {
    let mut out = format!(
        "Host logical CPUs: {cpus} | CPU scale: {}\n\n",
        scale.label()
    );
    let active_applications: Vec<_> = tree
        .windows_applications
        .iter()
        .filter(|application| application.resource.cpu_percent > 0.0)
        .collect();
    if !active_applications.is_empty() {
        out.push_str("Windows applications\n");
        for (index, application) in active_applications.iter().enumerate() {
            let prefix = if index + 1 == active_applications.len() {
                "`-"
            } else {
                "|-"
            };
            let _ = writeln!(
                out,
                "{prefix} {:<11} {:<27} {:>7.2}%",
                "application",
                application.resource.name,
                scale.value(application.resource.cpu_percent, cpus)
            );
            let contributors: Vec<_> = application
                .processes
                .iter()
                .filter(|process| process.cpu_percent > 0.0)
                .collect();
            for (process_index, process) in contributors.iter().enumerate() {
                let child = if process_index + 1 == contributors.len() {
                    "`-"
                } else {
                    "|-"
                };
                let _ = writeln!(
                    out,
                    "   {child} {:<7} {:<27} {:>7.2}%  pid {}",
                    "process",
                    process.name,
                    scale.value(process.cpu_percent, cpus),
                    process.pid.unwrap_or_default()
                );
            }
        }
        out.push('\n');
    }
    for group in &tree.groups {
        let unresolved = if group.mapping_status == MappingStatus::Unresolved {
            " [session mapping unresolved]"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<42} {:>7.2}%{}",
            group.name,
            scale.value(group.cpu_percent, cpus),
            unresolved
        );
        for child in &group.children {
            let _ = writeln!(
                out,
                "|- {:<10} {:<27} {:>7.2}%",
                child.kind.as_str(),
                display_name(child),
                scale.value(child.cpu_percent, cpus)
            );
            if matches!(
                child.environment,
                EnvironmentKind::Docker | EnvironmentKind::WslContainer
            ) {
                let container_groups = if child.environment == EnvironmentKind::Docker {
                    &tree.docker_groups
                } else {
                    &tree.wslc_groups
                };
                if let Some(docker) = container_groups
                    .iter()
                    .find(|docker| docker.container.id == child.id)
                {
                    for process in &docker.children {
                        let _ = writeln!(
                            out,
                            "|  |- {:<7} {:<24} {:>7.2}%",
                            "process",
                            process.name,
                            scale.value(process.cpu_percent, cpus)
                        );
                    }
                    let _ = writeln!(
                        out,
                        "|  `- {:<7} {:<24} {:>7.2}%",
                        "unattributed",
                        "",
                        scale.value(docker.unattributed_cpu_percent, cpus)
                    );
                }
            }
        }
        let _ = writeln!(
            out,
            "`- {:<10} {:<27} {:>7.2}%\n",
            "unattributed",
            "",
            scale.value(group.unattributed_cpu_percent, cpus)
        );
        if group.over_attributed_cpu_percent > 0.0 {
            let _ = writeln!(
                out,
                "   sampling skew (children exceed host by {:.2}%)\n",
                scale.value(group.over_attributed_cpu_percent, cpus)
            );
        }
    }
    if !tree.unmapped_children.is_empty() {
        out.push_str("Session mapping unresolved; resources remain ungrouped:\n");
        for child in &tree.unmapped_children {
            let _ = writeln!(
                out,
                "   {:<10} {:<27} {:>7.2}%",
                child.kind.as_str(),
                display_name(child),
                scale.value(child.cpu_percent, cpus)
            );
        }
    }
    let attached_docker_ids: std::collections::HashSet<_> = tree
        .groups
        .iter()
        .flat_map(|group| &group.children)
        .filter(|child| child.environment == EnvironmentKind::Docker)
        .map(|child| child.id.as_str())
        .collect();
    let independent: Vec<_> = tree
        .docker_groups
        .iter()
        .filter(|group| !attached_docker_ids.contains(group.container.id.as_str()))
        .collect();
    if !independent.is_empty() {
        out.push_str("Docker\n");
        for (index, docker) in independent.iter().enumerate() {
            let container_prefix = if index + 1 == independent.len() {
                "`-"
            } else {
                "|-"
            };
            let _ = writeln!(
                out,
                "{container_prefix} {:<10} {:<27} {:>7.2}%",
                "container",
                display_name(&docker.container),
                scale.value(docker.container.cpu_percent, cpus)
            );
            for process in &docker.children {
                let _ = writeln!(
                    out,
                    "   |- {:<7} {:<27} {:>7.2}%",
                    "process",
                    process.name,
                    scale.value(process.cpu_percent, cpus)
                );
            }
            let _ = writeln!(
                out,
                "   `- {:<7} {:<27} {:>7.2}%",
                "unattributed",
                "",
                scale.value(docker.unattributed_cpu_percent, cpus)
            );
            if docker.over_attributed_cpu_percent > 0.0 {
                let _ = writeln!(
                    out,
                    "      sampling skew (processes exceed container by {:.2}%)",
                    scale.value(docker.over_attributed_cpu_percent, cpus)
                );
            }
        }
    }
    out
}

fn display_name(row: &ResourceUsage) -> String {
    row.source.as_ref().map_or_else(
        || row.name.clone(),
        |source| {
            let source = if matches!(
                row.environment,
                EnvironmentKind::Docker | EnvironmentKind::WslContainer
            ) {
                source.chars().take(12).collect::<String>()
            } else {
                source.clone()
            };
            format!("[{source}] {}", row.name)
        },
    )
}
fn display_id(row: &ResourceUsage) -> String {
    if row.kind == crate::model::ResourceKind::Application {
        return "-".to_string();
    }
    row.pid
        .map_or_else(|| row.id.chars().take(12).collect(), |pid| pid.to_string())
}
fn env_name(environment: EnvironmentKind) -> &'static str {
    match environment {
        EnvironmentKind::Windows => "Windows",
        EnvironmentKind::Wsl => "WSL",
        EnvironmentKind::WslContainer => "WSLC",
        EnvironmentKind::Docker => "Docker",
    }
}
fn format_bytes(bytes: u64) -> String {
    let value = bytes as f64;
    if value >= 1024.0 * 1024.0 * 1024.0 {
        format!("{:.2}G", value / (1024.0 * 1024.0 * 1024.0))
    } else if value >= 1024.0 * 1024.0 {
        format!("{:.0}M", value / (1024.0 * 1024.0))
    } else if value >= 1024.0 {
        format!("{:.0}K", value / 1024.0)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
mod tests {
    use super::{display_name, flat, tree, CpuScale};
    use crate::attribution::{AttributionTree, DockerAttributionGroup};
    use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage, WindowsApplicationUsage};
    use crate::monitor::MonitorSnapshot;

    #[test]
    fn shortens_docker_container_source_in_text_output() {
        let row = ResourceUsage {
            environment: EnvironmentKind::Docker,
            source: Some(
                "68dae66282ff3087c9e2ad1e0ab1f59ef121080203c1fd0128363d7706df28a2".to_string(),
            ),
            kind: ResourceKind::Process,
            id: "container:33390".to_string(),
            pid: Some(33390),
            start_id: None,
            ppid: Some(1),
            name: "simx".to_string(),
            args: Some("simx".to_string()),
            cpu_percent: 8.06,
            memory_bytes: 25 * 1024 * 1024,
        };

        assert_eq!(display_name(&row), "[68dae66282ff] simx");
        assert_eq!(row.source.as_deref().unwrap().len(), 64);
    }

    fn snapshot(row: ResourceUsage) -> MonitorSnapshot {
        MonitorSnapshot {
            host_logical_cpu_count: 16,
            resources: vec![row.clone()],
            pid_resources: vec![row.clone()],
            tree: AttributionTree {
                host_logical_cpu_count: 16,
                groups: Vec::new(),
                unmapped_children: Vec::new(),
                docker_groups: vec![DockerAttributionGroup {
                    container: row,
                    children: Vec::new(),
                    unattributed_cpu_percent: 0.0,
                    over_attributed_cpu_percent: 0.0,
                }],
                wslc_groups: Vec::new(),
                windows_applications: Vec::new(),
            },
            warnings: Vec::new(),
        }
    }

    #[test]
    fn renders_flat_and_tree_in_the_selected_cpu_scale() {
        let row = ResourceUsage {
            environment: EnvironmentKind::Docker,
            source: None,
            kind: ResourceKind::Container,
            id: "container".into(),
            pid: None,
            start_id: None,
            ppid: None,
            name: "work".into(),
            args: None,
            cpu_percent: 6.25,
            memory_bytes: 0,
        };
        let snapshot = snapshot(row);

        let core_flat = flat(&snapshot, CpuScale::Core);
        let core_tree = tree(&snapshot, CpuScale::Core);
        assert!(core_flat.contains("1 core = 100%"));
        assert!(core_flat.contains("100.00%"));
        assert!(core_tree.contains("100.00%"));

        let host_flat = flat(&snapshot, CpuScale::Host);
        let host_tree = tree(&snapshot, CpuScale::Host);
        assert!(host_flat.contains("whole host = 100%"));
        assert!(host_flat.contains("  6.25%"));
        assert!(host_tree.contains("  6.25%"));
    }

    #[test]
    fn flat_application_and_other_rows_share_cpu_column_alignment() {
        let application = ResourceUsage {
            environment: EnvironmentKind::Windows,
            source: None,
            kind: ResourceKind::Application,
            id: "windows-app:test".into(),
            pid: None,
            start_id: None,
            ppid: None,
            name: "Test".into(),
            args: None,
            cpu_percent: 6.25,
            memory_bytes: 1,
        };
        let mut container = application.clone();
        container.environment = EnvironmentKind::Docker;
        container.kind = ResourceKind::Container;
        container.id = "container".into();
        let mut snapshot = snapshot(container.clone());
        snapshot.resources = vec![application.clone(), container];

        let output = flat(&snapshot, CpuScale::Host);
        let rows: Vec<_> = output
            .lines()
            .filter(|line| line.starts_with("Windows") || line.starts_with("Docker"))
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].find("  6.25%"), rows[1].find("  6.25%"));
    }

    #[test]
    fn tree_nests_windows_processes_under_the_application_total() {
        let mut process = ResourceUsage {
            environment: EnvironmentKind::Windows,
            source: None,
            kind: ResourceKind::Process,
            id: "42".into(),
            pid: Some(42),
            start_id: Some(42),
            ppid: None,
            name: "msedgewebview2".into(),
            args: None,
            cpu_percent: 2.0,
            memory_bytes: 1,
        };
        let mut application = process.clone();
        application.kind = ResourceKind::Application;
        application.id = "windows-app:ms-teams".into();
        application.pid = None;
        application.name = "Teams".into();
        let mut snapshot = snapshot(application.clone());
        snapshot.tree.windows_applications = vec![WindowsApplicationUsage {
            resource: application,
            processes: vec![process.clone()],
        }];

        let output = tree(&snapshot, CpuScale::Host);
        assert!(output.contains("Windows applications"));
        assert!(output.contains("application Teams"));
        assert!(output.contains("msedgewebview2"));
        assert!(output.contains("pid 42"));

        process.cpu_percent = 99.0;
        assert_eq!(
            snapshot.tree.windows_applications[0].resource.cpu_percent,
            2.0
        );
    }

    #[test]
    fn tree_hides_zero_cpu_windows_applications_without_hiding_json_model() {
        let mut application = ResourceUsage {
            environment: EnvironmentKind::Windows,
            source: None,
            kind: ResourceKind::Application,
            id: "windows-app:idle".into(),
            pid: None,
            start_id: None,
            ppid: None,
            name: "IdleApp".into(),
            args: None,
            cpu_percent: 0.0,
            memory_bytes: 1,
        };
        let mut snapshot = snapshot(application.clone());
        snapshot.tree.windows_applications = vec![WindowsApplicationUsage {
            resource: application.clone(),
            processes: Vec::new(),
        }];

        assert!(!tree(&snapshot, CpuScale::Host).contains("Windows applications"));
        assert_eq!(snapshot.tree.windows_applications.len(), 1);

        application.cpu_percent = 1.0;
        snapshot.tree.windows_applications[0].resource = application;
        assert!(tree(&snapshot, CpuScale::Host).contains("IdleApp"));
    }
}
