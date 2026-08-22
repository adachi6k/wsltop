use crate::model::{EnvironmentKind, ProcessKey, ProcessSample, Snapshot, WindowsSnapshot};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Deserialize)]
struct RawWindowsSnapshot {
    logical_cpu_count: u32,
    processes: Vec<RawWindowsProcess>,
}

#[derive(Debug, Deserialize)]
struct RawWindowsProcess {
    pid: u32,
    name: String,
    cpu_time_secs: f64,
    memory_bytes: u64,
}

pub fn snapshot(show_wsl_host: bool) -> Result<WindowsSnapshot, Box<dyn Error>> {
    // Get-Process CPU is cumulative processor time in seconds. Idle is excluded because
    // its CPU time increases while CPUs are idle and would invert the meaning of "usage".
    // vmmem/vmmemWSL/vmmemwslc-* are hidden by default to avoid double-counting WSL load.
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$cpuCount = [int](Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
if ($cpuCount -le 0) { $cpuCount = [Environment]::ProcessorCount }
$items = @(Get-Process | ForEach-Object {
    $cpu = $_.CPU
    if ($null -eq $cpu) { $cpu = 0.0 }
    [PSCustomObject]@{
        pid = [uint32]$_.Id
        name = [string]$_.ProcessName
        cpu_time_secs = [double]$cpu
        memory_bytes = [uint64]$_.WorkingSet64
    }
})
[PSCustomObject]@{
    logical_cpu_count = $cpuCount
    processes = $items
} | ConvertTo-Json -Compress -Depth 3
"#;

    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("failed to execute powershell.exe (is WSL interop enabled?): {e}"),
            )
        })?;

    if !output.status.success() {
        return Err(format!(
            "powershell.exe failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }

    let raw: RawWindowsSnapshot = serde_json::from_slice(&output.stdout)?;
    if raw.logical_cpu_count == 0 {
        return Err("Windows reported zero logical processors".into());
    }

    let mut processes = Vec::with_capacity(raw.processes.len());
    for process in raw.processes {
        if process.name.eq_ignore_ascii_case("idle") {
            continue;
        }
        if !show_wsl_host && is_wsl_host_process(&process.name) {
            continue;
        }

        processes.push(ProcessSample {
            key: ProcessKey {
                environment: EnvironmentKind::Windows,
                pid: process.pid,
                // Phase 0 does not request StartTime because access can fail for protected
                // processes. A negative CPU delta is treated as PID reuse by the sampler.
                start_id: 0,
            },
            name: process.name,
            cpu_time_secs: process.cpu_time_secs,
            memory_bytes: process.memory_bytes,
        });
    }

    Ok(WindowsSnapshot {
        snapshot: Snapshot {
            captured_at: Instant::now(),
            processes,
        },
        host_logical_cpu_count: raw.logical_cpu_count,
    })
}

fn is_wsl_host_process(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "vmmem" || name == "vmmemwsl" || name.starts_with("vmmemwslc-")
}

#[cfg(test)]
mod tests {
    use super::is_wsl_host_process;

    #[test]
    fn recognizes_wsl_host_processes() {
        assert!(is_wsl_host_process("vmmem"));
        assert!(is_wsl_host_process("VmmemWSL"));
        assert!(is_wsl_host_process("vmmemwslc-cli-adach"));
        assert!(!is_wsl_host_process("wsl"));
    }
}
