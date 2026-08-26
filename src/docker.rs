use crate::command;
use crate::model::{ContainerProcessUsage, EnvironmentKind, ResourceKind, ResourceUsage};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::process::Command;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct RawDockerStat {
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
pub struct DockerUsage {
    pub resources: Vec<ContainerProcessUsage>,
    pub warnings: Vec<String>,
}

pub fn usage(host_logical_cpu_count: u32) -> Result<DockerUsage, Box<dyn Error>> {
    let mut result = aggregate_usage(host_logical_cpu_count)?;
    populate_processes(&mut result, host_logical_cpu_count);
    Ok(result)
}

pub fn aggregate_usage(host_logical_cpu_count: u32) -> Result<DockerUsage, Box<dyn Error>> {
    if host_logical_cpu_count == 0 {
        return Ok(DockerUsage::default());
    }
    let output = match Command::new("docker")
        .args([
            "stats",
            "--no-stream",
            "--no-trunc",
            "--format",
            "{{json .}}",
        ])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(DockerUsage::default()),
        Err(error) => return Err(error.into()),
    };
    if !output.status.success() {
        if daemon_unavailable(&output.stderr) {
            return Ok(DockerUsage::default());
        }
        return Err(format!(
            "docker stats failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let resources = parse_stats(&output.stdout, host_logical_cpu_count)?;
    Ok(DockerUsage {
        resources: resources
            .into_iter()
            .map(|resource| ContainerProcessUsage {
                resource,
                processes: Vec::new(),
                host_pids: Vec::new(),
            })
            .collect(),
        warnings: Vec::new(),
    })
}

pub fn populate_processes(result: &mut DockerUsage, host_logical_cpu_count: u32) {
    for start in (0..result.resources.len()).step_by(4) {
        let end = (start + 4).min(result.resources.len());
        let targets: Vec<_> = result.resources[start..end]
            .iter()
            .map(|item| (item.resource.id.clone(), item.resource.name.clone()))
            .collect();
        let collected = std::thread::scope(|scope| {
            targets
                .iter()
                .map(|(id, _)| {
                    scope.spawn(move || {
                        container_processes(id, host_logical_cpu_count)
                            .map_err(|error| error.to_string())
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("Docker detail worker panicked".to_string()))
                })
                .collect::<Vec<_>>()
        });
        for (offset, processes) in collected.into_iter().enumerate() {
            match processes {
                Ok(processes) => result.resources[start + offset].processes = processes,
                Err(error) => result.warnings.push(format!(
                    "Docker container {} process attribution unavailable: {error}",
                    targets[offset].1
                )),
            }
        }
    }
}

fn container_processes(
    id: &str,
    host_logical_cpu_count: u32,
) -> Result<Vec<ResourceUsage>, Box<dyn Error>> {
    let output = command::output_with_timeout(
        Command::new("docker").args(["top", id, "-eo", "pid,ppid,pcpu,rss,comm,args"]),
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(format!(
            "docker top failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    parse_top(&output.stdout, id, host_logical_cpu_count)
}

fn parse_top(
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
        .ok_or("docker top output is empty")?
        .split_whitespace()
        .collect();
    if header.len() < 6
        || !header[0].eq_ignore_ascii_case("PID")
        || !header[1].eq_ignore_ascii_case("PPID")
        || !matches!(header[2].to_ascii_uppercase().as_str(), "%CPU" | "PCPU")
        || !header[3].eq_ignore_ascii_case("RSS")
    {
        return Err("unsupported docker top columns".into());
    }
    lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() < 6 {
                return Err(format!("malformed docker top process row: {line:?}").into());
            }
            let pid = fields[0].parse::<u32>()?;
            let ppid = fields[1].parse::<u32>()?;
            let cpu = fields[2].parse::<f64>()?;
            let rss_kib = fields[3].parse::<u64>()?;
            if !cpu.is_finite() || cpu < 0.0 {
                return Err("invalid docker top CPU percentage".into());
            }
            let command = fields[4];
            let args = fields[5..].join(" ");
            Ok(ResourceUsage {
                environment: EnvironmentKind::Docker,
                source: Some(container_id.to_string()),
                kind: ResourceKind::Process,
                id: format!("{container_id}:{pid}"),
                pid: Some(pid),
                start_id: None,
                ppid: Some(ppid),
                name: command.to_string(),
                args: Some(args),
                cpu_percent: cpu / host_cpu_count as f64,
                memory_bytes: rss_kib.saturating_mul(1024),
            })
        })
        .collect()
}

fn daemon_unavailable(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("failed to connect to the docker api")
        || message.contains("cannot connect to the docker daemon")
        || message.contains("error during connect")
        || message.contains("is the docker daemon running")
}

fn parse_stats(input: &[u8], host_cpu_count: u32) -> Result<Vec<ResourceUsage>, Box<dyn Error>> {
    let text = std::str::from_utf8(input)?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let raw: RawDockerStat = serde_json::from_str(line)?;
            Ok(ResourceUsage {
                environment: EnvironmentKind::Docker,
                source: None,
                kind: ResourceKind::Container,
                id: raw.id,
                pid: None,
                start_id: None,
                ppid: None,
                name: raw.name,
                args: None,
                cpu_percent: parse_percent(&raw.cpu_percent)? / host_cpu_count as f64,
                memory_bytes: parse_memory(&raw.memory_usage)?,
            })
        })
        .collect()
}

fn parse_percent(value: &str) -> Result<f64, Box<dyn Error>> {
    let value = value.trim().strip_suffix('%').ok_or("missing %")?;
    let value = value.trim().parse::<f64>()?;
    if !value.is_finite() || value < 0.0 {
        return Err("invalid CPU percentage".into());
    }
    Ok(value)
}

fn parse_memory(value: &str) -> Result<u64, Box<dyn Error>> {
    let used = value.split_once('/').map_or(value, |(used, _)| used).trim();
    let split = used.find(|character: char| !character.is_ascii_digit() && character != '.');
    let (number, unit) = split.map_or((used, "B"), |index| used.split_at(index));
    let amount = number.trim().parse::<f64>()?;
    let multiplier = match unit.trim() {
        "B" => 1.0,
        "kB" | "KB" => 1_000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "KiB" => 1024.0,
        "MiB" => 1024.0 * 1024.0,
        "GiB" => 1024.0 * 1024.0 * 1024.0,
        unit => return Err(format!("unsupported memory unit {unit:?}").into()),
    };
    if !amount.is_finite() || amount < 0.0 {
        return Err("invalid memory value".into());
    }
    Ok((amount * multiplier).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::{daemon_unavailable, parse_stats, parse_top};
    use crate::model::{EnvironmentKind, ResourceKind};

    #[test]
    fn parses_and_normalizes_docker_stats() {
        let input = br#"{"ID":"abcdef","Name":"web","CPUPerc":"32.00%","MemUsage":"12.5MiB / 1GiB"}
{"ID":"123456","Name":"db","CPUPerc":"0.00%","MemUsage":"1GB / 2GB"}"#;
        let rows = parse_stats(input, 16).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].environment, EnvironmentKind::Docker);
        assert_eq!(rows[0].kind, ResourceKind::Container);
        assert_eq!(rows[0].cpu_percent, 2.0);
        assert_eq!(rows[0].memory_bytes, 13_107_200);
    }

    #[test]
    fn recognizes_unavailable_docker_daemon_messages() {
        for message in [
            "failed to connect to the docker API at unix:///var/run/docker.sock",
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock",
            "error during connect: this error may indicate that the docker daemon is not running",
        ] {
            assert!(daemon_unavailable(message.as_bytes()));
        }
        assert!(!daemon_unavailable(b"docker stats: unknown flag --bad"));
    }

    #[test]
    fn parses_docker_top_and_normalizes_host_cpu() {
        let input =
            b"PID PPID %CPU RSS COMMAND COMMAND\n42 1 499.2 2048 cc1plus cc1plus -O2 source.cc\n";
        let rows = parse_top(input, "abcdef", 16).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].pid, Some(42));
        assert_eq!(rows[0].ppid, Some(1));
        assert_eq!(rows[0].name, "cc1plus");
        assert_eq!(rows[0].args.as_deref(), Some("cc1plus -O2 source.cc"));
        assert_eq!(rows[0].cpu_percent, 31.2);
        assert_eq!(rows[0].memory_bytes, 2_097_152);
    }

    #[test]
    fn rejects_malformed_or_unsupported_docker_top() {
        assert!(parse_top(b"PID COMMAND\n1 init\n", "id", 8).is_err());
        assert!(parse_top(b"PID PPID %CPU RSS COMMAND COMMAND\nbad row\n", "id", 8).is_err());
    }
}
