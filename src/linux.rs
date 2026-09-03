#[cfg(unix)]
use crate::linux_proc;
use crate::model::Snapshot;
#[cfg(unix)]
use crate::model::{EnvironmentKind, ProcessKey, ProcessSample};
use std::io;

#[cfg(unix)]
use std::{fs, time::Instant};

#[cfg(unix)]
pub fn snapshot() -> io::Result<Snapshot> {
    let clock_ticks = sysconf_positive(libc::_SC_CLK_TCK)? as f64;
    let page_size = sysconf_positive(libc::_SC_PAGESIZE)? as u64;
    let mut processes = Vec::new();

    for entry in fs::read_dir("/proc")? {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };
        let file_name = entry.file_name();
        let Some(pid_text) = file_name.to_str() else {
            continue;
        };
        let Ok(pid) = pid_text.parse::<u32>() else {
            continue;
        };

        if let Ok(sample) = read_process(pid, clock_ticks, page_size) {
            processes.push(sample);
        }
    }

    Ok(Snapshot {
        captured_at: Instant::now(),
        processes,
    })
}

#[cfg(windows)]
pub fn snapshot() -> io::Result<Snapshot> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "local /proc collection is unavailable on Windows",
    ))
}

#[cfg(unix)]
fn read_process(pid: u32, clock_ticks: f64, page_size: u64) -> io::Result<ProcessSample> {
    let stat_path = format!("/proc/{pid}/stat");
    let parsed = linux_proc::parse_stat(&fs::read_to_string(&stat_path)?, &stat_path)?;
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let name = linux_proc::cmdline_name(&cmdline).unwrap_or(parsed.command);

    Ok(ProcessSample {
        key: ProcessKey {
            environment: EnvironmentKind::Wsl,
            source: None,
            pid,
            start_id: parsed.start_ticks,
        },
        name,
        cpu_time_secs: (parsed.user_ticks + parsed.system_ticks) as f64 / clock_ticks,
        memory_bytes: parsed.resident_pages.saturating_mul(page_size),
    })
}

#[cfg(unix)]
fn sysconf_positive(name: libc::c_int) -> io::Result<i64> {
    let value = unsafe { libc::sysconf(name) };
    if value <= 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}
