use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::process::Command;

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

pub fn usage(host_logical_cpu_count: u32) -> Result<Vec<ResourceUsage>, Box<dyn Error>> {
    if host_logical_cpu_count == 0 {
        return Ok(Vec::new());
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
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if !output.status.success() {
        return Err(format!(
            "docker stats failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    parse_stats(&output.stdout, host_logical_cpu_count)
}

fn parse_stats(input: &[u8], host_cpu_count: u32) -> Result<Vec<ResourceUsage>, Box<dyn Error>> {
    let text = std::str::from_utf8(input)?;
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let raw: RawDockerStat = serde_json::from_str(line)?;
            Ok(ResourceUsage {
                environment: EnvironmentKind::Docker,
                kind: ResourceKind::Container,
                id: raw.id,
                pid: None,
                name: raw.name,
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
    use super::parse_stats;
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
}
