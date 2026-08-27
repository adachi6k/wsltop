use crate::command;
use crate::model::{EnvironmentKind, ProcessKey, ProcessSample, Snapshot, WindowsSnapshot};
use crate::windows_app::{WindowsMetadata, WindowsProcessMetadata};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static HOST_LOGICAL_CPU_COUNT: OnceLock<u32> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct RawWindowsSnapshot {
    logical_cpu_count: u32,
    logical_cpu_count_from_cim: bool,
    processes: Vec<RawWindowsProcess>,
}

#[derive(Debug, Deserialize)]
struct RawWindowsProcess {
    pid: u32,
    name: String,
    start_id: u64,
    cpu_time_secs: f64,
    memory_bytes: u64,
}

#[derive(Debug, Deserialize)]
struct RawMetadataSnapshot {
    processes: Vec<WindowsProcessMetadata>,
}

pub fn application_metadata() -> Result<WindowsMetadata, Box<dyn Error>> {
    let script = r#"
$ErrorActionPreference = 'Stop'
$items = @(Get-CimInstance Win32_Process | ForEach-Object {
    [PSCustomObject]@{
        pid = [uint32]$_.ProcessId
        parent_pid = [uint32]$_.ParentProcessId
        name = [string]$_.Name
        executable_path = if ($null -eq $_.ExecutablePath) { $null } else { [string]$_.ExecutablePath }
        command_line = if ($null -eq $_.CommandLine) { $null } else { [string]$_.CommandLine }
        start_id = if ($null -eq $_.CreationDate) { 0 } else { try { [uint64]([DateTime]$_.CreationDate).ToFileTimeUtc() } catch { 0 } }
    }
})
[PSCustomObject]@{ processes = $items } | ConvertTo-Json -Compress -Depth 3
"#;
    let output = command::output_with_timeout(
        Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", script]),
        Duration::from_secs(5),
    )
    .map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Windows application metadata command failed: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "Windows application metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let raw: RawMetadataSnapshot = serde_json::from_slice(&output.stdout)?;
    Ok(raw
        .processes
        .into_iter()
        .map(|process| (process.pid, process))
        .collect())
}

pub fn snapshot() -> Result<WindowsSnapshot, Box<dyn Error>> {
    // Get-Process CPU is cumulative processor time in seconds. Idle is excluded because
    // its CPU time increases while CPUs are idle and would invert the meaning of "usage".
    // vmmem/vmmemWSL/vmmemwslc-* are retained for attribution. The flat renderer
    // hides them by default to avoid double-counting WSL load.
    let script = snapshot_script(HOST_LOGICAL_CPU_COUNT.get().copied().unwrap_or(0));

    let output = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
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
    if raw.logical_cpu_count_from_cim {
        let _ = HOST_LOGICAL_CPU_COUNT.set(raw.logical_cpu_count);
    }

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
                start_id: process.start_id,
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

fn snapshot_script(cached_cpu_count: u32) -> String {
    r#"
$ErrorActionPreference = 'SilentlyContinue'
$cpuCount = [int]__WSLTOP_CPU_COUNT__
$cpuCountFromCim = $cpuCount -gt 0
if ($cpuCount -le 0) {
    $cpuCount = [int](Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors
    $cpuCountFromCim = $cpuCount -gt 0
}
if ($cpuCount -le 0) {
    $cpuCount = [int][Environment]::ProcessorCount
    $cpuCountFromCim = $false
}
if ($cpuCount -le 0) { throw 'failed to determine the Windows host logical CPU count' }
$items = @(Get-Process | ForEach-Object {
    $cpu = $_.CPU
    if ($null -eq $cpu) { $cpu = 0.0 }
    $startId = try { [uint64]$_.StartTime.ToFileTimeUtc() } catch { 0 }
    [PSCustomObject]@{
        pid = [uint32]$_.Id
        name = [string]$_.ProcessName
        start_id = $startId
        cpu_time_secs = [double]$cpu
        memory_bytes = [uint64]$_.WorkingSet64
    }
})
[PSCustomObject]@{
    logical_cpu_count = $cpuCount
    logical_cpu_count_from_cim = $cpuCountFromCim
    processes = $items
} | ConvertTo-Json -Compress -Depth 3
"#
    .replace("__WSLTOP_CPU_COUNT__", &cached_cpu_count.to_string())
}

#[cfg(test)]
mod tests {
    use super::snapshot_script;

    #[test]
    fn embeds_cached_cpu_count_without_powershell_command_arguments() {
        let script = snapshot_script(16);
        assert!(script.contains("$cpuCount = [int]16"));
        assert!(script.contains("logical_cpu_count_from_cim = $cpuCountFromCim"));
        assert!(script.contains("StartTime.ToFileTimeUtc()"));
        assert!(script.contains("start_id = $startId"));
        assert!(script.contains("[Environment]::ProcessorCount"));
        assert!(!script.contains("__WSLTOP_CPU_COUNT__"));
        assert!(!script.contains("$args"));
    }
}
