mod linux;
mod model;
mod sampler;
mod windows;

use crate::model::{EnvironmentKind, ProcessUsage};
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
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_args()?;

    let linux_before = linux::snapshot()?;
    let windows_before = if options.wsl_only {
        None
    } else {
        Some(windows::snapshot(options.show_wsl_host)?)
    };

    thread::sleep(options.interval);

    let linux_after = linux::snapshot()?;
    let windows_after = if options.wsl_only {
        None
    } else {
        Some(windows::snapshot(options.show_wsl_host)?)
    };

    let host_cpu_count = match &windows_after {
        Some(snapshot) => snapshot.host_logical_cpu_count,
        None => std::thread::available_parallelism()
            .map(|n| n.get() as u32)
            .unwrap_or(1),
    };

    let mut usage = sampler::calculate_usage(&linux_before, &linux_after, host_cpu_count);

    if let (Some(before), Some(after)) = (&windows_before, &windows_after) {
        if before.host_logical_cpu_count != after.host_logical_cpu_count {
            eprintln!(
                "warning: Windows logical CPU count changed during sampling ({} -> {})",
                before.host_logical_cpu_count, after.host_logical_cpu_count
            );
        }
        usage.extend(sampler::calculate_usage(
            &before.snapshot,
            &after.snapshot,
            host_cpu_count,
        ));
    }

    usage.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
    });
    usage.truncate(options.limit);

    if options.json {
        println!("{}", serde_json::to_string_pretty(&usage)?);
    } else {
        print_table(&usage, host_cpu_count, options.wsl_only);
    }

    Ok(())
}

fn print_table(rows: &[ProcessUsage], host_cpu_count: u32, wsl_only: bool) {
    if wsl_only {
        eprintln!(
            "warning: --wsl-only uses the WSL-visible logical CPU count ({host_cpu_count}); host-normalized CPU% requires Windows interop"
        );
    }

    println!("Host logical CPUs: {host_cpu_count}");
    println!("{:<7} {:>7} {:>9} {:>8}  {}", "ENV", "CPU%", "MEM", "PID", "COMMAND");
    println!("{}", "-".repeat(68));

    for row in rows {
        println!(
            "{:<7} {:>6.2}% {:>9} {:>8}  {}",
            env_name(row.environment),
            row.cpu_percent,
            format_bytes(row.memory_bytes),
            row.pid,
            row.name
        );
    }
}

fn env_name(environment: EnvironmentKind) -> &'static str {
    match environment {
        EnvironmentKind::Windows => "Windows",
        EnvironmentKind::Wsl => "WSL",
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
    };

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--once" => {}
            "--json" => options.json = true,
            "--show-wsl-host" => options.show_wsl_host = true,
            "--wsl-only" => options.wsl_only = true,
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
Unified Windows/WSL process CPU monitor (Phase 0)\n\n\
USAGE:\n    wsltop [OPTIONS]\n\n\
OPTIONS:\n    --once                 Take one sampled measurement (default behavior)\n    --json                 Emit JSON instead of a table\n    --limit N              Show at most N processes [default: 30]\n    --interval-ms N        Sampling interval in milliseconds [default: 1000]\n    --show-wsl-host        Include vmmem/vmmemWSL Windows rows (double-counts WSL)\n    --wsl-only             Skip Windows collector; CPU% is not host-normalized if WSL CPU-limited\n    -h, --help             Show this help\n"
    );
}
