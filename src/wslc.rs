use crate::command;
use crate::model::{ContainerProcessUsage, EnvironmentKind, ResourceKind, ResourceUsage};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct RawWslcStat {
    #[serde(rename = "CPUPerc")]
    cpu_percent: String,
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "MemUsage")]
    memory_usage: String,
    #[serde(rename = "Name")]
    name: String,
}

#[derive(Debug, Clone, Default)]
pub struct WslcUsage {
    pub resources: Vec<ResourceUsage>,
    pub process_resources: Vec<ContainerProcessUsage>,
    pub warnings: Vec<String>,
}

/// Return current WSLC container usage normalized to the Windows-host CPU scale.
///
/// `wslc stats` reports CPU in the container convention where one fully busy
/// logical CPU is approximately 100%. wsltop divides that value by the Windows
/// host logical CPU count so that all host logical CPUs busy is 100%.
pub fn usage(host_logical_cpu_count: u32) -> Result<WslcUsage, Box<dyn Error>> {
    let mut result = aggregate_usage(host_logical_cpu_count)?;
    populate_processes(&mut result, host_logical_cpu_count);
    Ok(result)
}

pub fn aggregate_usage(host_logical_cpu_count: u32) -> Result<WslcUsage, Box<dyn Error>> {
    if host_logical_cpu_count == 0 {
        return Ok(WslcUsage::default());
    }

    let output = match Command::new("wslc.exe")
        .args(["stats", "--format", "json", "--no-trunc"])
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(WslcUsage::default()),
        Err(e) => return Err(e.into()),
    };

    if !output.status.success() {
        return Err(format!(
            "wslc.exe stats failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let raw: Vec<RawWslcStat> = serde_json::from_slice(&output.stdout)?;
    let mut result = Vec::with_capacity(raw.len());
    let mut warnings = Vec::new();

    for container in raw {
        let cpu_native = match parse_percent(&container.cpu_percent) {
            Ok(value) => value,
            Err(e) => {
                warnings.push(format!(
                    "skipping WSLC container {}: invalid CPU percentage {:?}: {e}",
                    container.name, container.cpu_percent
                ));
                continue;
            }
        };

        let memory_bytes = match parse_memory_usage(&container.memory_usage) {
            Ok(value) => value,
            Err(e) => {
                warnings.push(format!(
                    "WSLC container {} has invalid memory usage {:?}: {e}",
                    container.name, container.memory_usage
                ));
                0
            }
        };

        result.push(ResourceUsage {
            environment: EnvironmentKind::WslContainer,
            source: None,
            kind: ResourceKind::Container,
            id: container.id,
            pid: None,
            start_id: None,
            ppid: None,
            name: container.name,
            args: None,
            cpu_percent: cpu_native / host_logical_cpu_count as f64,
            cpu_time_seconds: None,
            memory_bytes,
        });
    }

    Ok(WslcUsage {
        resources: result,
        process_resources: Vec::new(),
        warnings,
    })
}

pub fn populate_processes(result: &mut WslcUsage, host_logical_cpu_count: u32) {
    let previous = std::mem::take(&mut result.process_resources);
    result.process_resources.clear();
    for start in (0..result.resources.len()).step_by(4) {
        let end = (start + 4).min(result.resources.len());
        let targets = &result.resources[start..end];
        let collected = std::thread::scope(|scope| {
            targets
                .iter()
                .map(|resource| {
                    scope.spawn(move || {
                        container_processes(&resource.id, host_logical_cpu_count)
                            .map_err(|error| error.to_string())
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("WSLC detail worker panicked".to_string()))
                })
                .collect::<Vec<_>>()
        });
        for (resource, processes) in targets.iter().zip(collected) {
            let processes = processes.unwrap_or_else(|error| {
                result.warnings.push(format!(
                    "WSLC container {} process attribution unavailable: {error}",
                    resource.name
                ));
                previous
                    .iter()
                    .find(|old| old.resource.id == resource.id)
                    .map_or_else(Vec::new, |old| old.processes.clone())
            });
            result.process_resources.push(ContainerProcessUsage {
                resource: resource.clone(),
                processes,
                host_pids: Vec::new(),
            });
        }
    }
}

fn container_processes(
    id: &str,
    host_logical_cpu_count: u32,
) -> Result<Vec<ResourceUsage>, Box<dyn Error>> {
    let mut last_error = String::new();
    for columns in [
        "pid,ppid,pcpu,rss,time,comm,args",
        "pid,ppid,pcpu,rss,comm,args",
    ] {
        let output = command::output_with_timeout(
            Command::new("wslc.exe").args(["exec", id, "ps", "-eo", columns]),
            Duration::from_secs(5),
        )?;
        if output.status.success() {
            match parse_processes(&output.stdout, id, host_logical_cpu_count) {
                Ok(processes) => return Ok(processes),
                Err(error) => last_error = error.to_string(),
            }
        } else {
            last_error = format!(
                "wslc.exe exec ps failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Err(last_error.into())
}

fn parse_processes(
    input: &[u8],
    container_id: &str,
    host_cpu_count: u32,
) -> Result<Vec<ResourceUsage>, Box<dyn Error>> {
    if host_cpu_count == 0 {
        return Err("host logical CPU count is zero".into());
    }
    let text = std::str::from_utf8(input)?;
    let mut lines = text.lines();
    let header: Vec<_> = lines
        .next()
        .ok_or("WSLC ps output is empty")?
        .split_whitespace()
        .collect();
    if header.len() < 6
        || !header[0].eq_ignore_ascii_case("PID")
        || !header[1].eq_ignore_ascii_case("PPID")
        || header[2] != "%CPU"
        || !header[3].eq_ignore_ascii_case("RSS")
    {
        return Err("unsupported WSLC ps columns".into());
    }
    let has_time = header
        .get(4)
        .is_some_and(|column| column.eq_ignore_ascii_case("TIME"));
    let command_index = if has_time { 5 } else { 4 };
    let args_index = command_index + 1;
    lines
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() <= args_index {
                return Some(Err(format!("malformed WSLC process row: {line:?}").into()));
            }
            let pid = match fields[0].parse::<u32>() {
                Ok(pid) => pid,
                Err(error) => return Some(Err(error.into())),
            };
            let ppid = match fields[1].parse::<u32>() {
                Ok(ppid) => ppid,
                Err(error) => return Some(Err(error.into())),
            };
            // Exclude the short-lived ps injected by this collector.
            if ppid == 0 && fields[command_index] == "ps" {
                return None;
            }
            let cpu = match fields[2].parse::<f64>() {
                Ok(cpu) if cpu.is_finite() && cpu >= 0.0 => cpu,
                _ => return Some(Err("invalid WSLC process CPU percentage".into())),
            };
            let rss_kib = match fields[3].parse::<u64>() {
                Ok(rss) => rss,
                Err(error) => return Some(Err(error.into())),
            };
            let cpu_time_seconds = if has_time {
                match parse_cpu_time(fields[4]) {
                    Ok(value) => Some(value),
                    Err(error) => return Some(Err(error)),
                }
            } else {
                None
            };
            Some(Ok(ResourceUsage {
                environment: EnvironmentKind::WslContainer,
                source: Some(container_id.to_string()),
                kind: ResourceKind::Process,
                id: format!("{container_id}:{pid}"),
                pid: Some(pid),
                start_id: None,
                ppid: Some(ppid),
                name: fields[command_index].to_string(),
                args: Some(fields[args_index..].join(" ")),
                cpu_percent: cpu / host_cpu_count as f64,
                cpu_time_seconds,
                memory_bytes: rss_kib.saturating_mul(1024),
            }))
        })
        .collect()
}

fn parse_cpu_time(value: &str) -> Result<f64, Box<dyn Error>> {
    let (days, clock) = value
        .split_once('-')
        .map_or(Ok((0_u64, value)), |(days, clock)| {
            Ok::<_, Box<dyn Error>>((days.parse::<u64>()?, clock))
        })?;
    let fields: Vec<_> = clock.split(':').collect();
    let seconds = match fields.as_slice() {
        [minutes, seconds] => minutes.parse::<u64>()? as f64 * 60.0 + seconds.parse::<f64>()?,
        [hours, minutes, seconds] => {
            hours.parse::<u64>()? as f64 * 3600.0
                + minutes.parse::<u64>()? as f64 * 60.0
                + seconds.parse::<f64>()?
        }
        _ => return Err("invalid process CPU time".into()),
    } + days as f64 * 86_400.0;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err("invalid process CPU time".into());
    }
    Ok(seconds)
}

fn parse_percent(value: &str) -> Result<f64, Box<dyn Error>> {
    let number = value
        .trim()
        .strip_suffix('%')
        .ok_or("percentage does not end in %")?
        .trim()
        .parse::<f64>()?;
    if !number.is_finite() || number < 0.0 {
        return Err("percentage is not a finite non-negative number".into());
    }
    Ok(number)
}

fn parse_memory_usage(value: &str) -> Result<u64, Box<dyn Error>> {
    let used = value
        .split_once('/')
        .map(|(used, _limit)| used)
        .unwrap_or(value)
        .trim();
    parse_size_bytes(used)
}

fn parse_size_bytes(value: &str) -> Result<u64, Box<dyn Error>> {
    let mut parts = value.split_whitespace();
    let amount = parts.next().ok_or("missing size amount")?.parse::<f64>()?;
    let unit = parts.next().unwrap_or("B");
    if parts.next().is_some() {
        return Err("unexpected extra tokens in size".into());
    }
    if !amount.is_finite() || amount < 0.0 {
        return Err("size is not a finite non-negative number".into());
    }

    let multiplier = match unit {
        "B" => 1.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        "TiB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "kB" | "KB" => 1000.0,
        "MB" => 1000.0 * 1000.0,
        "GB" => 1000.0 * 1000.0 * 1000.0,
        "TB" => 1000.0 * 1000.0 * 1000.0 * 1000.0,
        _ => return Err(format!("unsupported size unit {unit:?}").into()),
    };

    let bytes = amount * multiplier;
    if bytes > u64::MAX as f64 {
        return Err("size exceeds u64".into());
    }
    Ok(bytes.round() as u64)
}

#[cfg(test)]
mod tests {
    use super::{
        parse_cpu_time, parse_memory_usage, parse_percent, parse_processes, parse_size_bytes,
    };

    #[test]
    fn parses_wslc_cpu_percent() {
        assert!((parse_percent("100.29%").unwrap() - 100.29).abs() < 1e-9);
        assert!((parse_percent("3.02%").unwrap() - 3.02).abs() < 1e-9);
    }

    #[test]
    fn parses_wslc_memory_usage() {
        assert_eq!(
            parse_memory_usage("497.73 MiB / 15.56 GiB").unwrap(),
            (497.73_f64 * 1024.0 * 1024.0).round() as u64
        );
        assert_eq!(parse_memory_usage("0 B / 15.56 GiB").unwrap(), 0);
    }

    #[test]
    fn parses_binary_sizes() {
        assert_eq!(parse_size_bytes("1 KiB").unwrap(), 1024);
        assert_eq!(parse_size_bytes("1 GiB").unwrap(), 1024 * 1024 * 1024);
    }

    #[test]
    fn parses_wslc_processes_and_excludes_collector_ps() {
        let input = b"PID PPID %CPU RSS COMMAND COMMAND\n1 0 16.0 1024 cc1plus cc1plus -O2\n99 0 20.0 3864 ps ps -eo pid,ppid,pcpu,rss,comm,args\n";
        let rows = parse_processes(input, "abc", 16).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "cc1plus");
        assert_eq!(rows[0].cpu_percent, 1.0);
        assert_eq!(rows[0].memory_bytes, 1024 * 1024);
        assert_eq!(rows[0].cpu_time_seconds, None);
    }

    #[test]
    fn parses_wslc_process_cpu_time() {
        let input =
            b"PID PPID %CPU RSS TIME COMMAND COMMAND\n1 0 16.0 1024 12:34 cc1plus cc1plus -O2\n";
        let rows = parse_processes(input, "abc", 16).unwrap();
        assert_eq!(rows[0].cpu_time_seconds, Some(754.0));
        assert_eq!(parse_cpu_time("01:02:03").unwrap(), 3_723.0);
    }
}
