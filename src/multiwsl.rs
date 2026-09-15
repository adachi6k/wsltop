use crate::model::{EnvironmentKind, ProcessKey, ProcessSample, Snapshot};
use std::error::Error;
use std::io::Write;
use std::process::Command;
use std::process::{Output, Stdio};
use std::time::Instant;

// Longer than Linux TASK_COMM_LEN (16 bytes including the terminator), so a
// task name containing newlines cannot impersonate the system-section boundary.
const SYSTEM_MARKER: &str = "\nWSLTOP_SYSTEM_COUNTERS_BEGIN\n";

fn wsl_command() -> Command {
    #[allow(unused_mut)]
    let mut command = Command::new("wsl.exe");
    // A collector may still be starting/exiting when the TUI restores its
    // console. Do not let wsl.exe share and change the caller's console modes.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    command
}

pub fn running_distros() -> Result<Vec<String>, Box<dyn Error>> {
    list_distros(&["--list", "--running", "--quiet"])
}

#[cfg(windows)]
pub fn default_distro() -> Result<Option<String>, Box<dyn Error>> {
    let output = run_wsl_script(None, "printf '%s' \"$WSL_DISTRO_NAME\"")?;
    if !output.status.success() {
        return Err(format!(
            "wsl.exe default distro discovery failed: {}",
            decode_wsl_text(&output.stderr).trim()
        )
        .into());
    }
    let name = decode_wsl_text(&output.stdout).trim().to_string();
    Ok((!name.is_empty()).then_some(name))
}

fn list_distros(args: &[&str]) -> Result<Vec<String>, Box<dyn Error>> {
    let output = wsl_command().args(args).output()?;
    if !output.status.success() {
        return Err(format!(
            "wsl.exe distro discovery failed: {}",
            decode_wsl_text(&output.stderr).trim()
        )
        .into());
    }
    Ok(decode_wsl_text(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn snapshot(distro: &str, source: Option<&str>) -> Result<Snapshot, Box<dyn Error>> {
    let script = snapshot_script(source.is_none());
    let output = run_wsl_script(Some(distro), &script)?;
    if !output.status.success() {
        return Err(format!(
            "remote /proc collection for {distro} failed with {}: {}",
            output.status,
            decode_wsl_text(&output.stderr).trim()
        )
        .into());
    }
    parse_snapshot_bytes(source, &output.stdout)
}

fn snapshot_script(primary: bool) -> String {
    let script = "clock_ticks=$(getconf CLK_TCK) || exit; page_size=$(getconf PAGESIZE) || exit; printf '%s %s\\n' \"$clock_ticks\" \"$page_size\" || exit; for d in /proc/[0-9]*; do [ -r \"$d/stat\" ] && cat \"$d/stat\" 2>/dev/null || :; done";
    if primary {
        format!("{script}; printf '%s' '{SYSTEM_MARKER}'; grep '^cpu' /proc/stat 2>/dev/null || :; printf 'UPTIME '; cat /proc/uptime 2>/dev/null || :; printf 'BOOT '; cat /proc/sys/kernel/random/boot_id 2>/dev/null || :")
    } else {
        script.into()
    }
}

fn run_wsl_script(distro: Option<&str>, script: &str) -> Result<Output, Box<dyn Error>> {
    let mut command = wsl_command();
    if let Some(distro) = distro {
        command.args(["-d", distro]);
    }
    // Passing a shell program as a `sh -c` argument is unreliable because wsl.exe applies
    // Windows command-line parsing before forwarding it. stdin preserves the script exactly.
    let mut child = command
        .args(["--", "sh"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("failed to open wsl.exe stdin")?
        .write_all(script.as_bytes())?;
    Ok(child.wait_with_output()?)
}

fn parse_snapshot(source: Option<&str>, text: &str) -> Result<Snapshot, Box<dyn Error>> {
    let (text, system) = text.split_once(SYSTEM_MARKER).unwrap_or((text, ""));
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
                source: source.map(str::to_string),
                pid,
                start_id,
            },
            name: stat[open + 1..close].to_string(),
            cpu_time_secs: (utime + stime) as f64 / ticks,
            memory_bytes: (rss.max(0) as u64).saturating_mul(page_size),
        });
    }
    Ok(Snapshot {
        system_cpu: system.split_once("UPTIME ").and_then(|(stat, rest)| {
            let (uptime, boot) = rest.split_once("BOOT ")?;
            crate::linux_cpu::Sample::parse(stat, uptime, boot, ticks)
        }),
        captured_at: Instant::now(),
        processes,
    })
}

fn parse_snapshot_bytes(source: Option<&str>, bytes: &[u8]) -> Result<Snapshot, Box<dyn Error>> {
    parse_snapshot(source, &String::from_utf8_lossy(bytes))
}

fn decode_wsl_text(bytes: &[u8]) -> String {
    if bytes.iter().skip(1).step_by(2).any(|byte| *byte == 0) {
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
    #[test]
    fn shared_counters_are_requested_only_for_primary_distribution() {
        assert!(super::snapshot_script(true).contains(super::SYSTEM_MARKER));
        assert!(!super::snapshot_script(false).contains(super::SYSTEM_MARKER));
        assert!(!super::snapshot_script(false).contains("/proc/uptime"));
    }

    #[cfg(unix)]
    #[test]
    fn missing_optional_system_files_preserve_process_collection() {
        for path in [
            "/proc/stat",
            "/proc/uptime",
            "/proc/sys/kernel/random/boot_id",
        ] {
            let script =
                super::snapshot_script(true).replace(path, "/wsltop-nonexistent-system-counter");
            let output = std::process::Command::new("sh")
                .args(["-c", &script])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "optional {path} must not fail collection"
            );
            let snapshot = super::parse_snapshot_bytes(None, &output.stdout).unwrap();
            assert!(!snapshot.processes.is_empty());
            assert!(snapshot.system_cpu.is_none());
        }
    }
    #[test]
    fn remote_system_counters_are_optional_and_separate_from_process_rows() {
        let old = super::parse_snapshot(
            None,
            &format!(
                "100 4096\n{}cpu 100 0 0 0 0 0 0\ncpu0 0\nUPTIME 10 0\nBOOT boot-a\n",
                super::SYSTEM_MARKER
            ),
        )
        .unwrap();
        let new = super::parse_snapshot(
            None,
            &format!(
                "100 4096\n{}cpu 200 0 0 0 0 0 0\ncpu0 0\nUPTIME 11 0\nBOOT boot-a\n",
                super::SYSTEM_MARKER
            ),
        )
        .unwrap();
        assert!(new.processes.is_empty());
        assert_eq!(crate::linux_cpu::usage(&old, &new, 2), Some(50.0));
        let missing = super::parse_snapshot(None, "100 4096\n").unwrap();
        assert!(missing.system_cpu.is_none());
        let malformed = super::parse_snapshot(
            None,
            &format!("100 4096\n{}UPTIME x\nBOOT b\n", super::SYSTEM_MARKER),
        )
        .unwrap();
        assert!(malformed.system_cpu.is_none());
    }
    #[test]
    fn newline_task_name_cannot_truncate_later_rows_or_system_counters() {
        assert!(super::SYSTEM_MARKER.trim().len() > 16);
        let fields = "S 1 1 1 0 0 0 0 0 0 0 100 20 0 0 0 0 0 0 777 0 3";
        let processes = format!("100 4096\n41 (before) {fields}\n42 (\nWSLTOP_SYSTEM\n) {fields}\n43 (after) {fields}\n");
        for primary in [false, true] {
            let text = if primary {
                format!(
                    "{processes}{}cpu 100 0 0 0 0 0 0\ncpu0 0\nUPTIME 10 0\nBOOT boot-a\n",
                    super::SYSTEM_MARKER
                )
            } else {
                processes.clone()
            };
            let snapshot = parse_snapshot(None, &text).unwrap();
            // The line-oriented process parser skips the multiline record, but
            // it must retain subsequent records and the real system section.
            assert_eq!(
                snapshot
                    .processes
                    .iter()
                    .map(|row| row.key.pid)
                    .collect::<Vec<_>>(),
                vec![41, 43]
            );
            assert_eq!(snapshot.system_cpu.is_some(), primary);
        }
    }
    use super::{decode_wsl_text, parse_snapshot, parse_snapshot_bytes};
    #[test]
    fn parses_remote_process_and_source() {
        let stat = "42 (worker) S 1 1 1 0 0 0 0 0 0 0 100 20 0 0 0 0 0 0 777 0 3";
        let snapshot = parse_snapshot(Some("Ubuntu-2"), &format!("100 4096\n{stat}\n")).unwrap();
        assert_eq!(
            snapshot.processes[0].key.source.as_deref(),
            Some("Ubuntu-2")
        );
        assert_eq!(snapshot.processes[0].memory_bytes, 12288);
    }
    #[test]
    fn parses_primary_process_without_source() {
        let stat = "42 (worker) S 1 1 1 0 0 0 0 0 0 0 100 20 0 0 0 0 0 0 777 0 3";
        let snapshot = parse_snapshot(None, &format!("100 4096\n{stat}\n")).unwrap();
        assert_eq!(snapshot.processes[0].key.source, None);
    }
    #[test]
    fn parses_non_utf8_process_name_lossily() {
        let mut bytes = b"100 4096\n42 (work".to_vec();
        bytes.push(0xff);
        bytes.extend_from_slice(b"r) S 1 1 1 0 0 0 0 0 0 0 100 20 0 0 0 0 0 0 777 0 3\n");

        let snapshot = parse_snapshot_bytes(None, &bytes).unwrap();

        assert_eq!(snapshot.processes[0].name, "work\u{fffd}r");
    }
    #[test]
    fn decodes_utf16_distro_list() {
        let bytes: Vec<_> = "Ubuntu 日本語\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_wsl_text(&bytes), "Ubuntu 日本語\r\n");
    }
    #[test]
    fn preserves_utf8_wsl_output() {
        assert_eq!(
            decode_wsl_text("Ubuntu 日本語\r\n".as_bytes()),
            "Ubuntu 日本語\r\n"
        );
    }
}
