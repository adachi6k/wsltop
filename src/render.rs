use crate::attribution::{AttributionTree, MappingStatus};
use crate::model::{EnvironmentKind, ResourceUsage};
use crate::monitor::MonitorSnapshot;
use std::fmt::Write;

pub fn flat(snapshot: &MonitorSnapshot) -> String {
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
        "Host logical CPUs: {}",
        snapshot.host_logical_cpu_count
    );
    let _ = writeln!(
        out,
        "{:<7} {:<9} {:>7} {:>9} {:>12} COMMAND",
        "ENV", "TYPE", "CPU%", "MEM", "ID/PID"
    );
    let _ = writeln!(out, "{}", "-".repeat(84));
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
            "{:<7} {:<9} {:>6.2}% {:>9} {:>12}  {}",
            env_name(row.environment),
            row.kind.as_str(),
            row.cpu_percent,
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
                        "{:<7} {:<9} {:>6.2}% {:>9} {:>12}    |- {}",
                        "",
                        "process",
                        process.cpu_percent,
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
                        "{:<7} {:<9} {:>6.2}% {:>9} {:>12}    |- {} more processes",
                        "",
                        "processes",
                        omitted_cpu,
                        "-",
                        "-",
                        omitted.len()
                    );
                }
                let _ = writeln!(
                    out,
                    "{:<7} {:<9} {:>6.2}% {:>9} {:>12}    `- unattributed",
                    "", "residual", group.unattributed_cpu_percent, "-", "-"
                );
                if group.over_attributed_cpu_percent > 0.0 {
                    let _ = writeln!(
                        out,
                        "{:<7} {:<9} {:>6.2}% {:>9} {:>12}    `- over-attributed",
                        "", "residual", group.over_attributed_cpu_percent, "-", "-"
                    );
                }
            }
        }
    }
    out
}

pub fn tree(snapshot: &MonitorSnapshot) -> String {
    tree_model(&snapshot.tree, snapshot.host_logical_cpu_count)
}

fn tree_model(tree: &AttributionTree, cpus: u32) -> String {
    let mut out = format!("Host logical CPUs: {cpus}\n\n");
    for group in &tree.groups {
        let unresolved = if group.mapping_status == MappingStatus::Unresolved {
            " [session mapping unresolved]"
        } else {
            ""
        };
        let _ = writeln!(
            out,
            "{:<42} {:>7.2}%{}",
            group.name, group.cpu_percent, unresolved
        );
        for child in &group.children {
            let _ = writeln!(
                out,
                "|- {:<10} {:<27} {:>7.2}%",
                child.kind.as_str(),
                display_name(child),
                child.cpu_percent
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
                            "process", process.name, process.cpu_percent
                        );
                    }
                    let _ = writeln!(
                        out,
                        "|  `- {:<7} {:<24} {:>7.2}%",
                        "unattributed", "", docker.unattributed_cpu_percent
                    );
                }
            }
        }
        let _ = writeln!(
            out,
            "`- {:<10} {:<27} {:>7.2}%\n",
            "unattributed", "", group.unattributed_cpu_percent
        );
        if group.over_attributed_cpu_percent > 0.0 {
            let _ = writeln!(
                out,
                "   sampling skew (children exceed host by {:.2}%)\n",
                group.over_attributed_cpu_percent
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
                child.cpu_percent
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
                docker.container.cpu_percent
            );
            for process in &docker.children {
                let _ = writeln!(
                    out,
                    "   |- {:<7} {:<27} {:>7.2}%",
                    "process", process.name, process.cpu_percent
                );
            }
            let _ = writeln!(
                out,
                "   `- {:<7} {:<27} {:>7.2}%",
                "unattributed", "", docker.unattributed_cpu_percent
            );
            if docker.over_attributed_cpu_percent > 0.0 {
                let _ = writeln!(
                    out,
                    "      sampling skew (processes exceed container by {:.2}%)",
                    docker.over_attributed_cpu_percent
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
    use super::display_name;
    use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};

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
            ppid: Some(1),
            name: "simx".to_string(),
            args: Some("simx".to_string()),
            cpu_percent: 8.06,
            memory_bytes: 25 * 1024 * 1024,
        };

        assert_eq!(display_name(&row), "[68dae66282ff] simx");
        assert_eq!(row.source.as_deref().unwrap().len(), 64);
    }
}
