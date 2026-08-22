use crate::model::{EnvironmentKind, ProcessKey, ProcessSample, Snapshot};
use std::error::Error;
use std::process::Command;
use std::time::Instant;

pub fn snapshots() -> Result<Vec<(String, Snapshot)>, Box<dyn Error>> {
    let current = std::env::var("WSL_DISTRO_NAME").ok();
    let output = Command::new("wsl.exe")
        .args(["--list", "--running", "--quiet"])
        .output()?;
    if !output.status.success() {
        return Err("wsl.exe distro discovery failed".into());
    }
    let names = decode_wsl_text(&output.stdout);
    let mut result = Vec::new();
    for name in names.lines().map(str::trim).filter(|name| !name.is_empty()) {
        if current.as_deref() == Some(name) {
            continue;
        }
        if let Ok(snapshot) = snapshot(name) {
            result.push((name.to_string(), snapshot));
        }
    }
    Ok(result)
}

fn snapshot(distro: &str) -> Result<Snapshot, Box<dyn Error>> {
    let script = "printf '%s %s\\n' \"$(getconf CLK_TCK)\" \"$(getconf PAGESIZE)\"; for d in /proc/[0-9]*; do [ -r \"$d/stat\" ] && cat \"$d/stat\"; done";
    let output = Command::new("wsl.exe")
        .args(["-d", distro, "--", "sh", "-c", script])
        .output()?;
    if !output.status.success() {
        return Err("remote /proc collection failed".into());
    }
    parse_snapshot(distro, &String::from_utf8(output.stdout)?)
}

fn parse_snapshot(distro: &str, text: &str) -> Result<Snapshot, Box<dyn Error>> {
    let mut lines = text.lines();
    let header: Vec<_> = lines
        .next()
        .ok_or("missing header")?
        .split_whitespace()
        .collect();
    let ticks = header
        .first()
        .ok_or("missing clock ticks")?
        .parse::<f64>()?;
    let page_size = header.get(1).ok_or("missing page size")?.parse::<u64>()?;
    let mut processes = Vec::new();
    for stat in lines {
        let Some(open) = stat.find('(') else { continue };
        let Some(close) = stat.rfind(')') else {
            continue;
        };
        let Ok(pid) = stat[..open].trim().parse::<u32>() else {
            continue;
        };
        let fields: Vec<_> = stat[close + 1..].split_whitespace().collect();
        if fields.len() <= 21 {
            continue;
        }
        let Ok(utime) = fields[11].parse::<u64>() else {
            continue;
        };
        let Ok(stime) = fields[12].parse::<u64>() else {
            continue;
        };
        let Ok(start_id) = fields[19].parse::<u64>() else {
            continue;
        };
        let Ok(rss) = fields[21].parse::<i64>() else {
            continue;
        };
        processes.push(ProcessSample {
            key: ProcessKey {
                environment: EnvironmentKind::Wsl,
                source: Some(distro.to_string()),
                pid,
                start_id,
            },
            name: stat[open + 1..close].to_string(),
            cpu_time_secs: (utime + stime) as f64 / ticks,
            memory_bytes: (rss.max(0) as u64).saturating_mul(page_size),
        });
    }
    Ok(Snapshot {
        captured_at: Instant::now(),
        processes,
    })
}

fn decode_wsl_text(bytes: &[u8]) -> String {
    if bytes.iter().step_by(2).skip(1).any(|byte| *byte == 0) {
        let words: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        String::from_utf16_lossy(&words)
    } else {
        String::from_utf8_lossy(bytes).replace('\0', "")
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_wsl_text, parse_snapshot};
    #[test]
    fn parses_remote_process_and_source() {
        let stat = "42 (worker) S 1 1 1 0 0 0 0 0 0 0 100 20 0 0 0 0 0 0 777 0 3";
        let snapshot = parse_snapshot("Ubuntu-2", &format!("100 4096\n{stat}\n")).unwrap();
        assert_eq!(
            snapshot.processes[0].key.source.as_deref(),
            Some("Ubuntu-2")
        );
        assert_eq!(snapshot.processes[0].memory_bytes, 12288);
    }
    #[test]
    fn decodes_utf16_distro_list() {
        let bytes: Vec<_> = "Ubuntu\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_wsl_text(&bytes), "Ubuntu\r\n");
    }
}
