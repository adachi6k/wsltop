mod attribution;
mod docker;
mod linux;
mod model;
mod sampler;
mod windows;
mod wslc;

use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};
use std::cmp::Ordering;
use std::env;
use std::error::Error;
use std::thread;
use std::time::Duration;

#[derive(Debug)]
struct Options {
    interval: Duration,
    limit: usize,
    json: bool,
    show_wsl_host: bool,
    wsl_only: bool,
    no_wslc: bool,
    hide_infra: bool,
    tree: bool,
    no_docker: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_args()?;

    let linux_before = linux::snapshot()?;
    let windows_before = if options.wsl_only {
        None
    } else {
        Some(windows::snapshot()?)
    };

    thread::sleep(options.interval);

    let linux_after = linux::snapshot()?;
    let windows_after = if options.wsl_only {
        None
    } else {
        Some(windows::snapshot()?)
    };

    let host_cpu_count = match &windows_after {
        Some(snapshot) => snapshot.host_logical_cpu_count,
        None => std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1),
    };

    let linux_usage = sampler::calculate_usage(&linux_before, &linux_after, host_cpu_count);
    let mut windows_usage = Vec::new();

    if let (Some(before), Some(after)) = (&windows_before, &windows_after) {
        if before.host_logical_cpu_count != after.host_logical_cpu_count {
            eprintln!(
                "warning: Windows logical CPU count changed during sampling ({} -> {})",
                before.host_logical_cpu_count, after.host_logical_cpu_count
            );
        }
        windows_usage = sampler::calculate_usage(&before.snapshot, &after.snapshot, host_cpu_count);
    }

    let mut wslc_usage = Vec::new();
    if !options.wsl_only && !options.no_wslc {
        match wslc::usage(host_cpu_count) {
            Ok(rows) => wslc_usage = rows,
            Err(e) => eprintln!("warning: WSLC collector unavailable: {e}"),
        }
    }

    let mut docker_usage = Vec::new();
    if !options.no_docker {
        match docker::usage(host_cpu_count) {
            Ok(rows) => docker_usage = rows,
            Err(error) => eprintln!("warning: Docker collector unavailable: {error}"),
        }
    }

    if options.tree {
        let hosts: Vec<_> = windows_usage
            .iter()
            .filter(|row| attribution::is_host_resource(row))
            .cloned()
            .collect();
        let mut tree = attribution::build_tree_with_docker(
            host_cpu_count,
            &hosts,
            &linux_usage,
            &wslc_usage,
            &docker_usage,
        );
        if options.hide_infra {
            attribution::hide_infra(&mut tree);
        }
        if options.json {
            println!("{}", serde_json::to_string_pretty(&tree)?);
        } else {
            print_tree(&tree, options.wsl_only);
        }
        return Ok(());
    }

    let mut usage = linux_usage;
    usage.extend(windows_usage);
    usage.extend(wslc_usage);
    usage.extend(docker_usage.into_iter().map(|item| item.resource));
    prepare_flat_usage(
        &mut usage,
        options.show_wsl_host,
        options.hide_infra,
        options.limit,
    );

    if options.json {
        println!("{}", serde_json::to_string_pretty(&usage)?);
    } else {
        print_table(&usage, host_cpu_count, options.wsl_only);
    }

    Ok(())
}

fn prepare_flat_usage(
    usage: &mut Vec<ResourceUsage>,
    show_wsl_host: bool,
    hide_infra: bool,
    limit: usize,
) {
    if !show_wsl_host {
        usage.retain(|row| !attribution::is_host_resource(row));
    }
    if hide_infra {
        usage.retain(|row| row.kind != ResourceKind::Infra);
    }
    usage.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
    });
    usage.truncate(limit);
}

fn print_tree(tree: &attribution::AttributionTree, wsl_only: bool) {
    if wsl_only {
        eprintln!(
            "warning: --wsl-only cannot collect Windows host resources; attribution mapping is unresolved"
        );
    }

    println!("Host logical CPUs: {}", tree.host_logical_cpu_count);
    println!();

    for group in &tree.groups {
        let unresolved = if group.mapping_status == attribution::MappingStatus::Unresolved {
            " [session mapping unresolved]"
        } else {
            ""
        };
        println!(
            "{:<42} {:>7.2}%{}",
            group.name, group.cpu_percent, unresolved
        );
        for child in &group.children {
            println!(
                "|- {:<10} {:<27} {:>7.2}%",
                child.kind.as_str(),
                child.name,
                child.cpu_percent
            );
            if child.environment == EnvironmentKind::Docker {
                if let Some(docker) = tree
                    .docker_groups
                    .iter()
                    .find(|docker| docker.container.id == child.id)
                {
                    for process in &docker.children {
                        println!(
                            "|  |- {:<7} {:<24} {:>7.2}%",
                            "process", process.name, process.cpu_percent
                        );
                    }
                    println!(
                        "|  `- {:<7} {:<24} {:>7.2}%",
                        "unattributed", "", docker.unattributed_cpu_percent
                    );
                }
            }
        }
        println!(
            "`- {:<10} {:<27} {:>7.2}%",
            "unattributed", "", group.unattributed_cpu_percent
        );
        if group.over_attributed_cpu_percent > 0.0 {
            println!(
                "   sampling skew (children exceed host by {:.2}%)",
                group.over_attributed_cpu_percent
            );
        }
        println!();
    }

    if !tree.unmapped_children.is_empty() {
        println!("Session mapping unresolved; resources remain ungrouped:");
        for child in &tree.unmapped_children {
            println!(
                "   {:<10} {:<27} {:>7.2}%",
                child.kind.as_str(),
                child.name,
                child.cpu_percent
            );
        }
    }
}

fn print_table(rows: &[ResourceUsage], host_cpu_count: u32, wsl_only: bool) {
    if wsl_only {
        eprintln!(
            "warning: --wsl-only uses the WSL-visible logical CPU count ({host_cpu_count}); host-normalized CPU% requires Windows interop"
        );
    }

    println!("Host logical CPUs: {host_cpu_count}");
    println!(
        "{:<7} {:<9} {:>7} {:>9} {:>12} COMMAND",
        "ENV", "TYPE", "CPU%", "MEM", "ID/PID"
    );
    println!("{}", "-".repeat(84));

    for row in rows {
        println!(
            "{:<7} {:<9} {:>6.2}% {:>9} {:>12}  {}",
            env_name(row.environment),
            row.kind.as_str(),
            row.cpu_percent,
            format_bytes(row.memory_bytes),
            display_id(row),
            row.name
        );
    }
}

fn display_id(row: &ResourceUsage) -> String {
    match row.pid {
        Some(pid) => pid.to_string(),
        None => row.id.chars().take(12).collect(),
    }
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
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;

    if value >= GIB {
        format!("{:.2}G", value / GIB)
    } else if value >= MIB {
        format!("{:.0}M", value / MIB)
    } else if value >= KIB {
        format!("{:.0}K", value / KIB)
    } else {
        format!("{bytes}B")
    }
}

fn parse_args() -> Result<Options, Box<dyn Error>> {
    let mut options = Options {
        interval: Duration::from_millis(1000),
        limit: 30,
        json: false,
        show_wsl_host: false,
        wsl_only: false,
        no_wslc: false,
        hide_infra: false,
        tree: false,
        no_docker: false,
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--once" => {}
            "--json" => options.json = true,
            "--show-wsl-host" => options.show_wsl_host = true,
            "--wsl-only" => options.wsl_only = true,
            "--no-wslc" => options.no_wslc = true,
            "--hide-infra" => options.hide_infra = true,
            "--tree" => options.tree = true,
            "--no-docker" => options.no_docker = true,
            "--interval-ms" => {
                let value = args.next().ok_or("--interval-ms requires a value")?;
                let millis = value.parse::<u64>()?;
                if millis < 100 {
                    return Err("--interval-ms must be at least 100".into());
                }
                options.interval = Duration::from_millis(millis);
            }
            "--limit" => {
                let value = args.next().ok_or("--limit requires a value")?;
                options.limit = value.parse::<usize>()?;
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }

    Ok(options)
}

fn print_help() {
    println!(
        "wsltop 0.1.0\n\n\
Unified Windows/WSL/WSLC/Docker CPU monitor (Phase 2)\n\n\
USAGE:\n    wsltop [OPTIONS]\n\n\
OPTIONS:\n    --once                 Take one sampled measurement (default behavior)\n    --json                 Emit JSON instead of a table\n    --tree                 Emit the WSL/WSLC CPU attribution tree\n    --limit N              Show at most N flat resources [default: 30]\n    --interval-ms N        Sampling interval in milliseconds [default: 1000]\n    --show-wsl-host        Include raw vmmem/vmmemWSL/vmmemwslc-* rows in flat output\n    --wsl-only             Skip Windows and WSLC collectors\n    --no-wslc              Disable automatic WSLC container collection\n    --no-docker            Disable automatic Docker container collection\n    --hide-infra           Hide infrastructure resource rows\n    -h, --help             Show this help\n"
    );
}

#[cfg(test)]
mod tests {
    use super::prepare_flat_usage;
    use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};

    fn resource(environment: EnvironmentKind, kind: ResourceKind, name: &str) -> ResourceUsage {
        ResourceUsage {
            environment,
            kind,
            id: name.to_string(),
            pid: Some(1),
            name: name.to_string(),
            cpu_percent: 1.0,
            memory_bytes: 0,
        }
    }

    #[test]
    fn flat_output_hides_hosts_by_default_but_keeps_resource_types() {
        let mut rows = vec![
            resource(EnvironmentKind::Windows, ResourceKind::Host, "vmmemwsl"),
            resource(EnvironmentKind::Wsl, ResourceKind::Infra, "plan9"),
            resource(
                EnvironmentKind::WslContainer,
                ResourceKind::Container,
                "container",
            ),
        ];

        prepare_flat_usage(&mut rows, false, false, 30);

        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.kind == ResourceKind::Infra));
        assert!(rows.iter().any(|row| row.kind == ResourceKind::Container));
    }

    #[test]
    fn flat_output_can_show_raw_hosts() {
        let mut rows = vec![resource(
            EnvironmentKind::Windows,
            ResourceKind::Host,
            "vmmemwsl",
        )];
        prepare_flat_usage(&mut rows, true, false, 30);
        assert_eq!(rows.len(), 1);
    }
}
