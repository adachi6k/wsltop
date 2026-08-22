use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::process::Command;

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

/// Return current WSLC container usage normalized to the Windows-host CPU scale.
///
/// `wslc stats` reports CPU in the container convention where one fully busy
/// logical CPU is approximately 100%. wsltop divides that value by the Windows
/// host logical CPU count so that all host logical CPUs busy is 100%.
pub fn usage(host_logical_cpu_count: u32) -> Result<Vec<ResourceUsage>, Box<dyn Error>> {
    if host_logical_cpu_count == 0 {
        return Ok(Vec::new());
    }

    let output = match Command::new("wslc.exe")
        .args(["stats", "--format", "json", "--no-trunc"])
        .output()
    {
        Ok(output) => output,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
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

    for container in raw {
        let cpu_native = match parse_percent(&container.cpu_percent) {
            Ok(value) => value,
            Err(e) => {
                eprintln!(
                    "warning: skipping WSLC container {}: invalid CPU percentage {:?}: {e}",
                    container.name, container.cpu_percent
                );
                continue;
            }
        };

        let memory_bytes = match parse_memory_usage(&container.memory_usage) {
            Ok(value) => value,
            Err(e) => {
                eprintln!(
                    "warning: WSLC container {} has invalid memory usage {:?}: {e}",
                    container.name, container.memory_usage
                );
                0
            }
        };

        result.push(ResourceUsage {
            environment: EnvironmentKind::WslContainer,
            kind: ResourceKind::Container,
            id: container.id,
            pid: None,
            name: container.name,
            cpu_percent: cpu_native / host_logical_cpu_count as f64,
            memory_bytes,
        });
    }

    Ok(result)
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
    use super::{parse_memory_usage, parse_percent, parse_size_bytes};

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
}
