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
    process_timestamp: u64,
    cpu_accounting: Option<crate::cpu_accounting::Sample>,
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
    // PerfProc includes services whose Get-Process.CPU getter is access-denied.
    // Keep VM hosts for attribution; exclude only Idle from actual CPU usage.
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

    parse_snapshot(&output.stdout)
}

fn parse_snapshot(output: &[u8]) -> Result<WindowsSnapshot, Box<dyn Error>> {
    let raw: RawWindowsSnapshot = serde_json::from_slice(output)?;
    if raw.logical_cpu_count == 0 {
        return Err("Windows reported zero logical processors".into());
    }
    if raw.process_timestamp == 0 {
        return Err("Windows process counter timestamp is unavailable".into());
    }
    if raw.logical_cpu_count_from_cim {
        let _ = HOST_LOGICAL_CPU_COUNT.set(raw.logical_cpu_count);
    }

    let mut processes = Vec::with_capacity(raw.processes.len());
    for process in raw.processes {
        if process.name.eq_ignore_ascii_case("idle") {
            continue;
        }
        if !process.cpu_time_secs.is_finite() || process.cpu_time_secs < 0.0 {
            return Err("Windows process counter CPU time is invalid".into());
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
        process_timestamp: raw.process_timestamp,
        cpu_accounting: raw.cpu_accounting,
        snapshot: Snapshot {
            system_cpu: None,
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

pub fn calculate_usage(
    before: &WindowsSnapshot,
    after: &WindowsSnapshot,
) -> Vec<crate::model::ResourceUsage> {
    if before.host_logical_cpu_count != after.host_logical_cpu_count {
        return Vec::new();
    }
    let Some(elapsed) = after
        .process_timestamp
        .checked_sub(before.process_timestamp)
    else {
        return Vec::new();
    };
    crate::sampler::calculate_usage_with_elapsed(
        &before.snapshot,
        &after.snapshot,
        after.host_logical_cpu_count,
        elapsed as f64 / 10_000_000.0,
    )
}

pub fn cpu_breakdown(
    before: &WindowsSnapshot,
    after: &WindowsSnapshot,
) -> Option<crate::cpu_accounting::Breakdown> {
    if before.host_logical_cpu_count != after.host_logical_cpu_count {
        return None;
    }
    let native_total = after
        .host_cpu
        .zip(before.host_cpu)
        .and_then(|(a, b)| a.usage_since(b));
    after.cpu_accounting.as_ref()?.usage_since(
        before.cpu_accounting.as_ref()?,
        after.host_logical_cpu_count,
        native_total,
    )
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
$processNames = @{}
Get-Process | ForEach-Object { $processNames[[uint32]$_.Id] = [string]$_.ProcessName }
# Query failures and missing CPU counters must never become measured zeroes.
$perf = @(Get-CimInstance Win32_PerfRawData_PerfProc_Process -ErrorAction Stop)
$processTimestamp = ($perf | Where-Object { $_.Name -eq '_Total' }).Timestamp_Sys100NS
if ($null -eq $processTimestamp -or [uint64]$processTimestamp -eq 0) { throw 'Windows process counter timestamp unavailable' }
$items = @($perf | Where-Object { $_.Name -ne '_Total' -and $_.IDProcess -ne 0 } | ForEach-Object {
    if ($null -eq $_.PercentProcessorTime -or $null -eq $_.ElapsedTime -or $null -eq $_.WorkingSet -or $_.Timestamp_Sys100NS -ne $processTimestamp) {
        throw 'Windows process performance counters incomplete'
    }
    $processId = [uint32]$_.IDProcess
    # Win32_Process.CreationDate (application metadata) has microsecond precision.
    # Match that precision while retaining creation time in the PID identity.
    $startId = [uint64]$_.ElapsedTime
    $startId -= $startId % [uint64]10
    $processName = $processNames[$processId]
    if ($null -eq $processName) { $processName = [string]$_.Name -replace '#[0-9]+$', '' }
    [PSCustomObject]@{
        pid = $processId
        name = $processName
        start_id = $startId
        cpu_time_secs = [double]$_.PercentProcessorTime / 10000000.0
        memory_bytes = [uint64]$_.WorkingSet
    }
})
$hostCpu = $null
try {
    Add-Type -ErrorAction Stop -TypeDefinition @'
using System;
using System.Collections.Generic;
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
    [StructLayout(LayoutKind.Sequential)]
    struct RawCounter {
        public uint status, timeLow, timeHigh;
        public long first, second;
        public uint multiple;
    }
    [StructLayout(LayoutKind.Sequential)]
    struct RawItem { public IntPtr name; public RawCounter value; }
    public class CpuCounter { public string name; public ulong busy, total; }
    public class Partitions {
        public string kind = "hyper_v";
        public CpuCounter[] physical, root, guest;
    }
    [DllImport("pdh.dll", CharSet=CharSet.Unicode)]
    static extern uint PdhOpenQueryW(string source, UIntPtr data, out IntPtr query);
    [DllImport("pdh.dll", CharSet=CharSet.Unicode)]
    static extern uint PdhAddEnglishCounterW(IntPtr query, string path, UIntPtr data, out IntPtr counter);
    [DllImport("pdh.dll")]
    static extern uint PdhCollectQueryData(IntPtr query);
    [DllImport("pdh.dll", CharSet=CharSet.Unicode)]
    static extern uint PdhGetRawCounterArrayW(IntPtr counter, ref uint size, ref uint count, IntPtr items);
    [DllImport("pdh.dll")]
    static extern uint PdhCloseQuery(IntPtr query);
    static void Check(uint status) {
        if (status != 0) throw new Exception("PDH status " + status.ToString("X8"));
    }
    static CpuCounter[] ReadArray(IntPtr counter) {
        uint size=0, count=0;
        uint status=PdhGetRawCounterArrayW(counter, ref size, ref count, IntPtr.Zero);
        if (status == 0x800007D1) return new CpuCounter[0];
        if (status != 0x800007D2 && status != 0) Check(status);
        if (size == 0 || size > 16777216) throw new Exception("invalid PDH array size");
        IntPtr buffer=Marshal.AllocHGlobal((int)size);
        try {
            Check(PdhGetRawCounterArrayW(counter, ref size, ref count, buffer));
            int stride=Marshal.SizeOf(typeof(RawItem));
            if ((ulong)count*(ulong)stride > size) throw new Exception("invalid PDH array count");
            List<CpuCounter> result=new List<CpuCounter>();
            for (int i=0; i<count; i++) {
                RawItem item=(RawItem)Marshal.PtrToStructure(IntPtr.Add(buffer,i*stride),typeof(RawItem));
                string name=Marshal.PtrToStringUni(item.name);
                if (name == "_Total") continue;
                if (item.value.status > 1 || item.value.first < 0 || item.value.second <= 0) throw new Exception("invalid PDH CPU value");
                result.Add(new CpuCounter { name=name, busy=(ulong)item.value.first, total=(ulong)item.value.second });
            }
            return result.ToArray();
        } finally { Marshal.FreeHGlobal(buffer); }
    }
    public static Partitions ReadPartitions() {
        IntPtr query;
        Check(PdhOpenQueryW(null,UIntPtr.Zero,out query));
        try {
            IntPtr physical, root, guest;
            Check(PdhAddEnglishCounterW(query,@"\Hyper-V Hypervisor Logical Processor(*)\% Total Run Time",UIntPtr.Zero,out physical));
            Check(PdhAddEnglishCounterW(query,@"\Hyper-V Hypervisor Root Virtual Processor(*)\% Total Run Time",UIntPtr.Zero,out root));
            Check(PdhAddEnglishCounterW(query,@"\Hyper-V Hypervisor Virtual Processor(*)\% Total Run Time",UIntPtr.Zero,out guest));
            Check(PdhCollectQueryData(query));
            return new Partitions { physical=ReadArray(physical), root=ReadArray(root), guest=ReadArray(guest) };
        } finally { PdhCloseQuery(query); }
    }
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
[PSCustomObject]$accounting = $null
try {
    $computer = Get-CimInstance Win32_ComputerSystem -ErrorAction Stop
    if ($computer.HypervisorPresent -eq $false) {
        $accounting = [PSCustomObject]@{ kind = 'native' }
    } elseif ($computer.HypervisorPresent -eq $true) {
        # All three counter sets are captured by the same PDH query. English
        # counter paths are localized by PDH, including on Japanese Windows.
        $accounting = [WsltopSystemTimes]::ReadPartitions()
    }
} catch { }
[PSCustomObject]@{
    cpu_accounting = $accounting
    host_cpu = $hostCpu
    host_memory = $hostMemory
    logical_cpu_count = $cpuCount
    logical_cpu_count_from_cim = $cpuCountFromCim
    processes = $items
    process_timestamp = [uint64]$processTimestamp
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
    use std::time::Duration;

    fn process_snapshot(timestamp: u64, cpu: f64, start: u64) -> crate::model::WindowsSnapshot {
        let json = serde_json::json!({
            "logical_cpu_count": 16, "logical_cpu_count_from_cim": true,
            "process_timestamp": timestamp,
            "processes": [{"pid":4,"name":"System","start_id":start,
                "cpu_time_secs":cpu,"memory_bytes":4096}],
        });
        super::parse_snapshot(&serde_json::to_vec(&json).unwrap()).unwrap()
    }

    #[test]
    fn system_process_cpu_uses_provider_time_not_command_completion_time() {
        let before = process_snapshot(100_000_000, 30.0, 1);
        let mut after = process_snapshot(120_000_000, 34.0, 1);
        // Delayed serialization or command completion must not halve the rate.
        after.snapshot.captured_at = before.snapshot.captured_at + Duration::from_secs(4);
        let rows = super::calculate_usage(&before, &after);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cpu_percent, 12.5);
        assert_eq!(rows[0].cpu_time_seconds, Some(34.0));
        assert!(super::calculate_usage(&after, &before).is_empty());
        assert!(super::calculate_usage(&before, &before).is_empty());
        let replacement = process_snapshot(120_000_000, 40.0, 2);
        assert!(super::calculate_usage(&before, &replacement).is_empty());
        after.host_logical_cpu_count = 32;
        assert!(super::calculate_usage(&before, &after).is_empty());
    }

    #[test]
    fn missing_cpu_and_timestamp_fail_instead_of_becoming_zero() {
        for time in [serde_json::Value::Null, serde_json::json!(-1.0)] {
            let raw = serde_json::json!({
                "logical_cpu_count":16,"logical_cpu_count_from_cim":true,
                "process_timestamp":100,
                "processes":[{"pid":4,"name":"System","start_id":1,
                    "cpu_time_secs":time,"memory_bytes":4096}],
            });
            assert!(super::parse_snapshot(&serde_json::to_vec(&raw).unwrap()).is_err());
        }
        assert!(super::parse_snapshot(br#"{"logical_cpu_count":16,"logical_cpu_count_from_cim":true,"process_timestamp":0,"processes":[]}"#).is_err());
        assert!(super::parse_snapshot(
            br#"{"logical_cpu_count":16,"logical_cpu_count_from_cim":true,"processes":[]}"#
        )
        .is_err());
    }

    #[test]
    #[ignore = "requires live Windows performance counters through Windows or WSL interop"]
    fn live_windows_cpu_counters() {
        let mut before = super::snapshot().unwrap();
        let metadata = super::application_metadata().unwrap();
        let matching = before
            .snapshot
            .processes
            .iter()
            .filter(|p| {
                metadata
                    .get(&p.key.pid)
                    .is_some_and(|m| m.start_id == p.key.start_id)
            })
            .count();
        if matching <= before.snapshot.processes.len() / 2 {
            for p in before
                .snapshot
                .processes
                .iter()
                .filter(|p| metadata.contains_key(&p.key.pid))
                .take(8)
            {
                println!(
                    "identity {} {} perf={} metadata={}",
                    p.key.pid, p.name, p.key.start_id, metadata[&p.key.pid].start_id
                );
            }
        }
        assert!(
            matching > before.snapshot.processes.len() / 2,
            "process creation times must remain compatible with application metadata"
        );
        let mut successful = 0;
        for _ in 0..12 {
            std::thread::sleep(Duration::from_secs(3));
            let after = super::snapshot().unwrap();
            let rows = super::calculate_usage(&before, &after);
            assert!(rows.iter().any(|r| r.name == "System"));
            let breakdown = super::cpu_breakdown(&before, &after);
            println!(
                "{}",
                serde_json::json!({"cpu": breakdown,
                "system": rows.iter().find(|r|r.name == "System").map(|r|r.cpu_percent),
                "defender": rows.iter().find(|r|r.name == "MsMpEng").map(|r|r.cpu_percent),
                "rows":rows.len()})
            );
            if let Some(b) = breakdown {
                let [total, win, vm, other] = b.tenths();
                assert_eq!(total, win + vm + other);
                successful += 1;
            }
            before = after;
        }
        assert!(
            successful >= 10,
            "CPU partitions unavailable in too many live samples: {successful}/12"
        );
    }

    #[test]
    fn embeds_cached_cpu_count_without_powershell_command_arguments() {
        let script = snapshot_script(16);
        assert!(script.contains("$cpuCount = [int]16"));
        assert!(script.contains("logical_cpu_count_from_cim = $cpuCountFromCim"));
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
            r#"{"logical_cpu_count":16,"logical_cpu_count_from_cim":true,"process_timestamp":100,"processes":[],"host_memory":{"total_bytes":34359738368,"available_bytes":8589934592}}"#
        ).unwrap();
        assert_eq!(raw.host_memory.unwrap().used_bytes(), Some(25769803776));
        let raw: super::RawWindowsSnapshot = serde_json::from_str(
            r#"{"logical_cpu_count":16,"logical_cpu_count_from_cim":true,"process_timestamp":100,"processes":[],"host_memory":null}"#
        ).unwrap();
        assert!(raw.host_memory.is_none());
    }
}
