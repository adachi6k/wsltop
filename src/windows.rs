use crate::model::{EnvironmentKind, ProcessKey, ProcessSample, Snapshot, WindowsSnapshot};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::process::Command;
use std::sync::OnceLock;
use std::time::Instant;

static HOST_LOGICAL_CPU_COUNT: OnceLock<u32> = OnceLock::new();

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

pub fn snapshot() -> Result<WindowsSnapshot, Box<dyn Error>> {
    // Get-Process CPU is cumulative processor time in seconds. Idle is excluded because
    // its CPU time increases while CPUs are idle and would invert the meaning of "usage".
    // vmmem/vmmemWSL/vmmemwslc-* are retained for attribution. The flat renderer
    // hides them by default to avoid double-counting WSL load.
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$cpuCount = [int]$args[0]
if ($cpuCount -le 0) {
    $cpuCount = [int](Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
    if ($cpuCount -le 0) { $cpuCount = [Environment]::ProcessorCount }
}
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

    let cached_cpu_count = HOST_LOGICAL_CPU_COUNT
        .get()
        .copied()
        .unwrap_or(0)
        .to_string();
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
            &cached_cpu_count,
        ])
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
    let _ = HOST_LOGICAL_CPU_COUNT.set(raw.logical_cpu_count);

    let mut processes = Vec::with_capacity(raw.processes.len());
    for process in raw.processes {
        if process.name.eq_ignore_ascii_case("idle") {
            continue;
        }
        processes.push(ProcessSample {
            key: ProcessKey {
                environment: EnvironmentKind::Windows,
                source: None,
                pid: process.pid,
                // StartTime is not requested because access can fail for protected processes.
                // A negative CPU delta is treated as PID reuse by the sampler.
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
