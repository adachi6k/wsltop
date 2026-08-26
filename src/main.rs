mod attribution;
mod command;
mod docker;
mod linux;
mod model;
mod monitor;
mod multiwsl;
mod render;
mod sampler;
mod stream;
mod tui;
mod windows;
mod windows_app;
mod wslc;

use crate::monitor::{Monitor, MonitorConfig};
use crate::render::CpuScale;
use std::env;
use std::error::Error;
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
    interactive: bool,
    show_container_processes: bool,
    container_process_limit: usize,
    cpu_scale: CpuScale,
    cpu_scale_explicit: bool,
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_args()?;
    validate_options(&options)?;
    run(options)
}

fn validate_options(options: &Options) -> Result<(), Box<dyn Error>> {
    if options.interactive && options.json {
        return Err("--interactive cannot be combined with --json".into());
    }
    if options.json && options.cpu_scale_explicit && options.cpu_scale == CpuScale::Core {
        return Err("--json uses host-wide CPU values; --cpu-scale core is display-only".into());
    }
    Ok(())
}

fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let config = MonitorConfig {
        interval: options.interval,
        limit: options.limit,
        show_wsl_host: options.show_wsl_host,
        wsl_only: options.wsl_only,
        no_wslc: options.no_wslc,
        no_docker: options.no_docker,
        hide_infra: options.hide_infra,
        show_container_processes: options.show_container_processes,
        container_process_limit: options.container_process_limit,
    };
    if options.interactive {
        return tui::run(config, options.tree, options.cpu_scale);
    }

    let mut monitor = Monitor::new(config);
    let snapshot = monitor.sample()?;
    for warning in &snapshot.warnings {
        eprintln!("warning: {warning}");
    }
    if options.json {
        if options.tree {
            println!("{}", serde_json::to_string_pretty(&snapshot.tree)?);
        } else {
            println!("{}", serde_json::to_string_pretty(&snapshot.pid_resources)?);
        }
    } else if options.tree {
        print!("{}", render::tree(&snapshot, options.cpu_scale));
    } else {
        print!("{}", render::flat(&snapshot, options.cpu_scale));
    }

    Ok(())
}

fn parse_args() -> Result<Options, Box<dyn Error>> {
    parse_args_from(env::args().skip(1))
}

fn parse_args_from<I, S>(args: I) -> Result<Options, Box<dyn Error>>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
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
        interactive: false,
        show_container_processes: false,
        container_process_limit: 5,
        cpu_scale: CpuScale::Core,
        cpu_scale_explicit: false,
    };

    let mut args = args.into_iter().map(Into::into);
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
            "--show-container-processes" | "--show-docker-processes" => {
                options.show_container_processes = true
            }
            "--container-process-limit" | "--docker-process-limit" => {
                let value = args
                    .next()
                    .ok_or("--container-process-limit requires a value")?;
                options.container_process_limit = value.parse::<usize>()?;
                if options.container_process_limit == 0 {
                    return Err("--container-process-limit must be at least 1".into());
                }
            }
            "--cpu-scale" => {
                let value = args.next().ok_or("--cpu-scale requires core or host")?;
                options.cpu_scale = CpuScale::parse(&value)?;
                options.cpu_scale_explicit = true;
            }
            "-i" | "--interactive" => options.interactive = true,
            "-V" | "--version" => {
                println!("wsltop {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
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
        "wsltop {}\n\n\
Unified Windows, WSL, WSL Containers, and Docker resource monitor for WSL2\n\n\
USAGE:\n    wsltop [OPTIONS]\n\n\
OPTIONS:\n    --once                 Take one sampled measurement (default behavior)\n    -i, --interactive      Run the continuously updating terminal UI\n    --json                 Emit JSON instead of a table (not valid with --interactive)\n    --tree                 Show the CPU attribution tree (initial TUI view when interactive)\n    --limit N              Show at most N flat resources [default: 30]\n    --interval-ms N        Sampling/refresh interval in milliseconds [default: 1000]\n    --cpu-scale SCALE      CPU display scale: core or host [default: core]\n    --show-wsl-host        Include raw vmmem/vmmemWSL/vmmemwslc-* rows in flat views\n    --wsl-only             Skip Windows, additional distro, and WSLC collectors\n    --no-wslc              Disable automatic WSLC container collection\n    --no-docker            Disable automatic Docker container collection\n    --show-container-processes Include Docker/WSLC processes in flat output\n    --container-process-limit N Show at most N processes per container [default: 5]\n    --hide-infra           Hide infrastructure resource rows\n    -h, --help             Show this help\n    -V, --version          Show version\n",
        env!("CARGO_PKG_VERSION")
    );
}

#[cfg(test)]
mod tests {
    use super::{parse_args_from, validate_options};
    use crate::render::CpuScale;

    #[test]
    fn defaults_human_output_to_per_core_scale() {
        let options = parse_args_from(Vec::<String>::new()).unwrap();
        assert_eq!(options.cpu_scale, CpuScale::Core);
        assert!(!options.cpu_scale_explicit);
    }

    #[test]
    fn parses_host_cpu_scale() {
        let options = parse_args_from(["--cpu-scale", "host"]).unwrap();
        assert_eq!(options.cpu_scale, CpuScale::Host);
        assert!(options.cpu_scale_explicit);
    }

    #[test]
    fn rejects_invalid_cpu_scale() {
        let error = parse_args_from(["--cpu-scale", "machine"])
            .unwrap_err()
            .to_string();
        assert!(error.contains("expected core or host"));
    }

    #[test]
    fn keeps_json_host_wide() {
        let implicit = parse_args_from(["--json"]).unwrap();
        validate_options(&implicit).unwrap();

        let explicit_host = parse_args_from(["--json", "--cpu-scale", "host"]).unwrap();
        validate_options(&explicit_host).unwrap();

        let explicit_core = parse_args_from(["--json", "--cpu-scale", "core"]).unwrap();
        assert!(validate_options(&explicit_core).is_err());
    }
}
