mod attribution;
mod collector;
mod command;
mod docker;
mod header;
mod history;
// Read-only identity foundation for the forthcoming Query API (#24).
// It is intentionally not exposed through the compatibility CLI/JSON yet.
#[allow(dead_code)]
mod identity;
#[cfg(unix)]
mod linux;
#[cfg_attr(windows, allow(dead_code))]
mod linux_proc;
mod model;
mod monitor;
mod multiwsl;
mod query;
// Read-only API over retained snapshots; transport/refresh integration follows.
#[allow(dead_code)]
mod query_api;
mod render;
mod sampler;
// Snapshot lifecycle foundation; the external Query API is a later slice of #24.
#[allow(dead_code)]
mod snapshot_store;
mod stream;
mod summary;
mod tui;
mod windows;
mod windows_app;
mod wslc;

use crate::monitor::{Monitor, MonitorConfig};
use crate::query::{Sort, SortKey, SortOrder};
use crate::render::CpuScale;
use std::env;
use std::error::Error;
use std::time::Duration;

const DEFAULT_INTERVAL_MS: u64 = 3000;

#[derive(Debug)]
struct Options {
    header: header::HeaderMode,
    color: header::ColorMode,
    sort: Sort,
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
    container_processes_explicit: bool,
    container_process_limit: usize,
    cpu_scale: CpuScale,
    cpu_scale_explicit: bool,
    distro: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = parse_args()?;
    validate_options(&options)?;
    run(options)
}

fn validate_options(options: &Options) -> Result<(), Box<dyn Error>> {
    validate_options_for_platform(options, cfg!(windows))
}

fn validate_options_for_platform(
    options: &Options,
    windows_native: bool,
) -> Result<(), Box<dyn Error>> {
    if options.interactive && options.json {
        return Err("--interactive cannot be combined with --json".into());
    }
    if !windows_native && options.distro.is_some() {
        return Err("--distro is only supported by the Windows-native executable".into());
    }
    if options.json && options.cpu_scale_explicit && options.cpu_scale == CpuScale::Core {
        return Err("--json uses host-wide CPU values; --cpu-scale core is display-only".into());
    }
    Ok(())
}

fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let collect_windows_applications = needs_windows_applications(&options);
    let config = MonitorConfig {
        sort: options.sort,
        interval: options.interval,
        limit: options.limit,
        show_wsl_host: options.show_wsl_host,
        wsl_only: options.wsl_only,
        no_wslc: options.no_wslc,
        no_docker: options.no_docker,
        hide_infra: options.hide_infra,
        show_container_processes: options.show_container_processes,
        container_process_limit: options.container_process_limit,
        collect_windows_applications,
    };
    if options.interactive {
        return tui::run(
            config,
            options.distro,
            options.tree,
            options.cpu_scale,
            options.header,
            options.color,
        );
    }

    let mut monitor = Monitor::new(config, options.distro);
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

fn needs_windows_applications(options: &Options) -> bool {
    options.interactive || !options.json || options.tree
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
        header: Default::default(),
        color: Default::default(),
        sort: Sort::default(),
        interval: Duration::from_millis(DEFAULT_INTERVAL_MS),
        limit: 30,
        json: false,
        show_wsl_host: false,
        wsl_only: false,
        no_wslc: false,
        hide_infra: false,
        tree: false,
        no_docker: false,
        interactive: false,
        show_container_processes: true,
        container_processes_explicit: false,
        container_process_limit: 5,
        cpu_scale: CpuScale::Core,
        cpu_scale_explicit: false,
        distro: None,
    };

    let mut args = args.into_iter().map(Into::into);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--header" => {
                options.header = header::HeaderMode::parse(
                    &args.next().ok_or("--header requires classic or compact")?,
                )?;
            }
            "--color" => {
                options.color = header::ColorMode::parse(
                    &args
                        .next()
                        .ok_or("--color requires auto, always or never")?,
                )?;
            }
            "--sort" => {
                options.sort.key =
                    SortKey::parse(&args.next().ok_or("--sort requires cpu, memory or name")?)?;
            }
            "--sort-order" => {
                options.sort.order =
                    SortOrder::parse(&args.next().ok_or("--sort-order requires asc or desc")?)?;
            }
            "--once" => {}
            "--json" => options.json = true,
            "--show-wsl-host" => options.show_wsl_host = true,
            "--wsl-only" => options.wsl_only = true,
            "--distro" => {
                let value = args.next().ok_or("--distro requires a name")?;
                let value = value.trim();
                if value.is_empty() {
                    return Err("--distro requires a non-empty name".into());
                }
                options.distro = Some(value.to_string());
            }
            "--no-wslc" => options.no_wslc = true,
            "--hide-infra" => options.hide_infra = true,
            "--tree" => options.tree = true,
            "--no-docker" => options.no_docker = true,
            "--show-container-processes" | "--show-docker-processes" => {
                options.show_container_processes = true;
                options.container_processes_explicit = true;
            }
            "--hide-container-processes" => {
                options.show_container_processes = false;
                options.container_processes_explicit = true;
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

    if options.json && !options.container_processes_explicit {
        options.show_container_processes = false;
    }

    Ok(options)
}

fn print_help() {
    println!(
        "wsltop {}\n\n\
Unified Windows, WSL, WSL Containers, and Docker resource monitor for WSL2\n\n\
USAGE:\n    wsltop [OPTIONS]\n\n\
OPTIONS:\n    --once                 Take one sampled measurement (default behavior)\n    -i, --interactive      Run the continuously updating terminal UI\n    --json                 Emit JSON instead of a table (not valid with --interactive)\n    --tree                 Show the CPU attribution tree (initial TUI view when interactive)\n    --limit N              Show at most N flat resources [default: 30]\n    --interval-ms N        Sampling/refresh interval in milliseconds [default: {}]\n    --sort KEY            Sort resources by cpu, memory or name [default: cpu]\n    --sort-order ORDER    Sort direction: asc or desc [default: desc]\n    --cpu-scale SCALE      CPU display scale: core or host [default: core]\n    --show-wsl-host        Include raw vmmem/vmmemWSL/vmmemwslc-* rows in flat views\n    --distro NAME          Select the primary WSL distro (Windows-native only)\n    --wsl-only             Skip Windows, additional distro, and WSLC collectors\n    --no-wslc              Disable automatic WSLC container collection\n    --no-docker            Disable automatic Docker container collection\n    --show-container-processes Include Docker/WSLC processes (default for text/TUI)\n    --hide-container-processes Hide Docker/WSLC processes from flat output\n    --container-process-limit N Show at most N processes per container [default: 5]\n    --hide-infra           Hide infrastructure resource rows\n    -h, --help             Show this help\n    -V, --version          Show version\n",
        env!("CARGO_PKG_VERSION"),
        DEFAULT_INTERVAL_MS
    );
    println!("TUI DISPLAY:\n    --header MODE          compact (two lines, default) or classic (one line)\n    --color MODE           auto (default), always or never; auto honors NO_COLOR\n\nPress ? in the TUI for summary metrics and controls. Environment observations\nmay overlap (WSL can include Docker); they are not an additive host breakdown.");
}

#[cfg(test)]
mod tests {
    use super::{
        parse_args_from, validate_options, validate_options_for_platform, DEFAULT_INTERVAL_MS,
    };
    use crate::render::CpuScale;

    #[test]
    fn parses_tui_display_options() {
        let defaults = parse_args_from(Vec::<String>::new()).unwrap();
        assert_eq!(defaults.header, crate::header::HeaderMode::Compact);
        assert_eq!(defaults.color, crate::header::ColorMode::Auto);
        let options =
            parse_args_from(["--interactive", "--header", "classic", "--color", "never"]).unwrap();
        assert_eq!(options.header, crate::header::HeaderMode::Classic);
        assert_eq!(options.color, crate::header::ColorMode::Never);
        for args in [
            vec!["--header"],
            vec!["--header", "huge"],
            vec!["--color"],
            vec!["--color", "blue"],
        ] {
            assert!(parse_args_from(args).is_err());
        }
    }

    #[test]
    fn defaults_human_output_to_per_core_scale() {
        let options = parse_args_from(Vec::<String>::new()).unwrap();
        assert_eq!(options.cpu_scale, CpuScale::Core);
        assert_eq!(options.sort, crate::query::Sort::default());
        assert!(!options.cpu_scale_explicit);
        assert!(options.show_container_processes);
        assert_eq!(
            options.interval,
            std::time::Duration::from_millis(DEFAULT_INTERVAL_MS)
        );
    }

    #[test]
    fn parses_shared_sort_options_for_text_json_and_tui() {
        for mode in ["--once", "--json", "--interactive"] {
            let options =
                parse_args_from([mode, "--sort", "memory", "--sort-order", "asc"]).unwrap();
            assert_eq!(options.sort.key, crate::query::SortKey::Memory);
            assert_eq!(options.sort.order, crate::query::SortOrder::Asc);
            assert!(validate_options_for_platform(&options, true).is_ok());
        }
        for args in [
            vec!["--sort"],
            vec!["--sort", "invalid"],
            vec!["--sort-order"],
            vec!["--sort-order", "invalid"],
        ] {
            assert!(parse_args_from(args).is_err());
        }
    }

    #[test]
    fn container_processes_default_on_for_humans_and_off_for_flat_json() {
        assert!(
            parse_args_from(["--interactive"])
                .unwrap()
                .show_container_processes
        );
        assert!(
            !parse_args_from(["--hide-container-processes"])
                .unwrap()
                .show_container_processes
        );
        assert!(
            !parse_args_from(["--json"])
                .unwrap()
                .show_container_processes
        );
        assert!(
            parse_args_from(["--json", "--show-container-processes"])
                .unwrap()
                .show_container_processes
        );
    }

    #[test]
    fn parses_host_cpu_scale() {
        let options = parse_args_from(["--cpu-scale", "host"]).unwrap();
        assert_eq!(options.cpu_scale, CpuScale::Host);
        assert!(options.cpu_scale_explicit);
    }

    #[test]
    fn parses_primary_distro() {
        let options = parse_args_from(["--distro", "Ubuntu-24.04"]).unwrap();
        assert_eq!(options.distro.as_deref(), Some("Ubuntu-24.04"));
        let padded = parse_args_from(["--distro", "  Ubuntu-24.04  "]).unwrap();
        assert_eq!(padded.distro.as_deref(), Some("Ubuntu-24.04"));
        assert!(parse_args_from(["--distro", ""]).is_err());
        assert!(parse_args_from(["--distro"]).is_err());
    }

    #[test]
    fn platform_validation_limits_distro_to_windows_native() {
        let distro = parse_args_from(["--distro", "Ubuntu"]).unwrap();
        assert!(validate_options_for_platform(&distro, false).is_err());
        assert!(validate_options_for_platform(&distro, true).is_ok());

        let interactive = parse_args_from(["--interactive"]).unwrap();
        assert!(validate_options_for_platform(&interactive, false).is_ok());
        assert!(validate_options_for_platform(&interactive, true).is_ok());
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

    #[test]
    fn flat_json_does_not_request_windows_application_metadata() {
        let flat_json = parse_args_from(["--json"]).unwrap();
        assert!(!super::needs_windows_applications(&flat_json));

        let tree_json = parse_args_from(["--json", "--tree"]).unwrap();
        assert!(super::needs_windows_applications(&tree_json));
    }
}
