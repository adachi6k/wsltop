use crate::command;
use crate::model::{
    EnvironmentKind, HostCpuSample, HostMemory, ProcessKey, ProcessSample, Snapshot,
    WindowsSnapshot,
};
use crate::windows_app::{WindowsMetadata, WindowsProcessMetadata};
use serde::Deserialize;
use std::error::Error;
use std::io;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

static HOST_LOGICAL_CPU_COUNT: OnceLock<u32> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct RawWindowsSnapshot {
    logical_cpu_count: u32,
    logical_cpu_count_from_cim: bool,
    processes: Vec<RawWindowsProcess>,
    host_cpu: Option<RawHostCpuSample>,
    host_memory: Option<HostMemory>,
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

#[derive(Debug, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
enum RawHostCpuSample {
    SystemTimes { idle: u64, kernel: u64, user: u64 },
    PerfTotal { idle: u64, timestamp: u64 },
}

impl RawHostCpuSample {
    fn into_sample(self) -> Option<HostCpuSample> {
        match self {
            Self::SystemTimes { idle, kernel, user } => Some(HostCpuSample {
                idle,
                timestamp: kernel.checked_add(user)?,
            }),
            Self::PerfTotal { idle, timestamp } => Some(HostCpuSample { idle, timestamp }),
        }
    }
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
        command::CommandSpec::new(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", script],
        ),
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

    let output = command::output_with_timeout(
        command::CommandSpec::new(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", &script],
        ),
        Duration::from_secs(10),
    )
    .map_err(|e| io::Error::new(e.kind(), format!("failed to execute powershell.exe: {e}")))?;

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
        host_cpu: raw.host_cpu.and_then(RawHostCpuSample::into_sample),
        host_memory: raw
            .host_memory
            .filter(|memory| memory.used_bytes().is_some()),
    })
}

#[cfg(target_os = "windows")]
pub fn host_logical_cpu_count() -> Result<u32, Box<dyn Error>> {
    if let Some(count) = HOST_LOGICAL_CPU_COUNT.get().copied() {
        return Ok(count);
    }

    let output = command::output_with_timeout(
        command::CommandSpec::new(
            "powershell.exe",
            &[
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                host_logical_cpu_count_script(),
            ],
        ),
        Duration::from_secs(5),
    )
    .map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("Windows host logical CPU count command failed: {error}"),
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "Windows host logical CPU count query failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    let count = parse_host_logical_cpu_count(&output.stdout)?;
    let _ = HOST_LOGICAL_CPU_COUNT.set(count);
    Ok(count)
}

#[cfg(any(target_os = "windows", test))]
fn host_logical_cpu_count_script() -> &'static str {
    "$count = [int](Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors; if ($count -le 0) { throw 'failed to determine the Windows host logical CPU count' }; Write-Output $count"
}

#[cfg(any(target_os = "windows", test))]
fn parse_host_logical_cpu_count(output: &[u8]) -> Result<u32, Box<dyn Error>> {
    let count = String::from_utf8_lossy(output).trim().parse::<u32>()?;
    if count == 0 {
        return Err("Windows reported zero logical processors".into());
    }
    Ok(count)
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
$hostCpu = $null
try {
    Add-Type -ErrorAction Stop -TypeDefinition @'
using System.Runtime.InteropServices;
public static class WsltopSystemTimes {
    [StructLayout(LayoutKind.Sequential)]
    public struct MemoryStatus {
        public uint length, memoryLoad;
        public ulong totalPhys, availPhys, totalPageFile, availPageFile;
        public ulong totalVirtual, availVirtual, availExtendedVirtual;
    }
    [DllImport("kernel32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GlobalMemoryStatusEx(ref MemoryStatus status);
    [DllImport("kernel32.dll")]
    public static extern ushort GetActiveProcessorGroupCount();
    [DllImport("kernel32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool GetSystemTimes(out ulong idle, out ulong kernel, out ulong user);
}
'@
    if ([WsltopSystemTimes]::GetActiveProcessorGroupCount() -eq 1) {
        [uint64]$idle = 0; [uint64]$kernel = 0; [uint64]$user = 0
        if ([WsltopSystemTimes]::GetSystemTimes([ref]$idle, [ref]$kernel, [ref]$user)) {
            $hostCpu = [PSCustomObject]@{
                source = 'system_times'
                idle = $idle
                kernel = $kernel
                user = $user
            }
        }
    } else {
        # GetSystemTimes covers only the calling processor group; use the system total instead.
        $counter = Get-CimInstance Win32_PerfRawData_PerfOS_Processor -Filter "Name='_Total'" -OperationTimeoutSec 1 -ErrorAction Stop
        if ($null -ne $counter.PercentProcessorTime -and $null -ne $counter.Timestamp_Sys100NS) {
            $hostCpu = [PSCustomObject]@{
                source = 'perf_total'
                idle = [uint64]$counter.PercentProcessorTime
                timestamp = [uint64]$counter.Timestamp_Sys100NS
            }
        }
    }
} catch { }
$hostMemory = $null
try {
    $memory = New-Object WsltopSystemTimes+MemoryStatus
    $memory.length = [uint32][System.Runtime.InteropServices.Marshal]::SizeOf($memory)
    if ([WsltopSystemTimes]::GlobalMemoryStatusEx([ref]$memory)) {
        $hostMemory = [PSCustomObject]@{
            total_bytes = $memory.totalPhys
            available_bytes = $memory.availPhys
        }
    }
} catch { }
[PSCustomObject]@{
    host_cpu = $hostCpu
    host_memory = $hostMemory
    logical_cpu_count = $cpuCount
    logical_cpu_count_from_cim = $cpuCountFromCim
    processes = $items
} | ConvertTo-Json -Compress -Depth 3
"#
    .replace("__WSLTOP_CPU_COUNT__", &cached_cpu_count.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        host_logical_cpu_count_script, parse_host_logical_cpu_count, snapshot_script,
        RawHostCpuSample,
    };

    #[test]
    fn embeds_cached_cpu_count_without_powershell_command_arguments() {
        let script = snapshot_script(16);
        assert!(script.contains("$cpuCount = [int]16"));
        assert!(script.contains("logical_cpu_count_from_cim = $cpuCountFromCim"));
        assert!(script.contains("StartTime.ToFileTimeUtc()"));
        assert!(script.contains("start_id = $startId"));
        assert!(script.contains("[Environment]::ProcessorCount"));
        assert!(script.contains("source = 'system_times'"));
        assert!(script.contains("source = 'perf_total'"));
        assert!(script.contains("GetActiveProcessorGroupCount() -eq 1"));
        assert!(script.contains("Win32_PerfRawData_PerfOS_Processor"));
        assert!(!script.contains("__WSLTOP_CPU_COUNT__"));
        assert!(!script.contains("$args"));
    }

    #[test]
    fn host_cpu_count_query_uses_cim_without_process_limited_fallback() {
        let script = host_logical_cpu_count_script();
        assert!(script.contains("Get-CimInstance Win32_ComputerSystem"));
        assert!(script.contains("NumberOfLogicalProcessors"));
        assert!(!script.contains("[Environment]::ProcessorCount"));
    }

    #[test]
    fn parses_positive_host_cpu_count() {
        assert_eq!(parse_host_logical_cpu_count(b"128\r\n").unwrap(), 128);
        assert!(parse_host_logical_cpu_count(b"0\r\n").is_err());
        assert!(parse_host_logical_cpu_count(b"unknown\r\n").is_err());
    }

    #[test]
    fn converts_single_group_system_times_to_host_cpu_sample() {
        let sample = RawHostCpuSample::SystemTimes {
            idle: 100,
            kernel: 200,
            user: 50,
        }
        .into_sample()
        .unwrap();
        assert_eq!(sample.idle, 100);
        assert_eq!(sample.timestamp, 250);
    }

    #[test]
    fn rejects_overflowing_single_group_system_times() {
        assert!(RawHostCpuSample::SystemTimes {
            idle: 100,
            kernel: u64::MAX,
            user: 1,
        }
        .into_sample()
        .is_none());
    }

    #[test]
    fn converts_multi_group_perf_total_to_host_cpu_sample() {
        let sample = RawHostCpuSample::PerfTotal {
            idle: 123,
            timestamp: 456,
        }
        .into_sample()
        .unwrap();
        assert_eq!(sample.idle, 123);
        assert_eq!(sample.timestamp, 456);
    }

    #[test]
    fn deserializes_unavailable_host_cpu_as_none() {
        #[derive(serde::Deserialize)]
        struct Wrapper {
            host_cpu: Option<RawHostCpuSample>,
        }

        let wrapper: Wrapper = serde_json::from_str(r#"{"host_cpu":null}"#).unwrap();
        assert!(wrapper.host_cpu.is_none());
    }

    #[test]
    fn host_memory_handles_full_free_missing_and_invalid_samples() {
        use crate::model::HostMemory;
        for (total, available, expected) in [
            (1024, 0, Some(1024)),
            (1024, 1024, Some(0)),
            (1024, 256, Some(768)),
            (0, 0, None),
            (1024, 2048, None),
        ] {
            assert_eq!(
                HostMemory {
                    total_bytes: total,
                    available_bytes: available
                }
                .used_bytes(),
                expected
            );
        }
        let raw: super::RawWindowsSnapshot = serde_json::from_str(
            r#"{"logical_cpu_count":16,"logical_cpu_count_from_cim":true,"processes":[],"host_memory":{"total_bytes":34359738368,"available_bytes":8589934592}}"#
        ).unwrap();
        assert_eq!(raw.host_memory.unwrap().used_bytes(), Some(25769803776));
        let raw: super::RawWindowsSnapshot = serde_json::from_str(
            r#"{"logical_cpu_count":16,"logical_cpu_count_from_cim":true,"processes":[],"host_memory":null}"#
        ).unwrap();
        assert!(raw.host_memory.is_none());
    }
}
