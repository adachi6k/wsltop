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
        system_cpu: fs::read_to_string("/proc/stat").ok().and_then(|stat| {
            crate::linux_cpu::Sample::parse(
                &stat,
                &fs::read_to_string("/proc/uptime").ok()?,
                &fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?,
                clock_ticks,
            )
        }),
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
    let stat = fs::read_to_string(&stat_path)?;
    let parsed = linux_proc::parse_stat(&stat, &stat_path)?;
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let name = linux_proc::cmdline_name(&cmdline).unwrap_or_else(|| parsed.command.to_string());
    let build_source = fs::read_to_string(format!("/proc/{pid}/cgroup"))
        .ok()
        .and_then(|cgroup| crate::buildkit::source(&cgroup, None))
        .filter(|_| {
            // Bracket cgroup membership with the process generation, so PID
            // reuse cannot attach an unrelated process to a build.
            fs::read_to_string(&stat_path).ok().is_some_and(|after| {
                linux_proc::parse_stat(&after, &stat_path)
                    .is_ok_and(|after| after.start_ticks == parsed.start_ticks)
            })
        });

    Ok(ProcessSample {
        key: ProcessKey {
            environment: if build_source.is_some() {
                EnvironmentKind::Docker
            } else {
                EnvironmentKind::Wsl
            },
            source: build_source,
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
