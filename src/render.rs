use crate::attribution::{AttributionTree, MappingStatus};
use crate::model::{EnvironmentKind, ResourceUsage};
use crate::monitor::MonitorSnapshot;
use std::fmt::Write;

pub fn flat(snapshot: &MonitorSnapshot) -> String {
    let mut out = String::new();
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
            if child.environment == EnvironmentKind::Docker {
                if let Some(docker) = tree
                    .docker_groups
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
    out
}

fn display_name(row: &ResourceUsage) -> String {
    row.source.as_ref().map_or_else(
        || row.name.clone(),
        |source| format!("[{source}] {}", row.name),
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
