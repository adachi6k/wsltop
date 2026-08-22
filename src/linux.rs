use crate::model::{EnvironmentKind, ProcessKey, ProcessSample, Snapshot};
use std::fs;
use std::io;
use std::path::Path;
use std::time::Instant;

pub fn snapshot() -> io::Result<Snapshot> {
    let clock_ticks = sysconf_positive(libc::_SC_CLK_TCK)? as f64;
    let page_size = sysconf_positive(libc::_SC_PAGESIZE)? as u64;
    let mut processes = Vec::new();

    for entry in fs::read_dir("/proc")? {
        let entry = match entry {
            Ok(v) => v,
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

fn read_process(pid: u32, clock_ticks: f64, page_size: u64) -> io::Result<ProcessSample> {
    let stat_path = format!("/proc/{pid}/stat");
    let stat = fs::read_to_string(&stat_path)?;

    let open = stat.find('(').ok_or_else(|| invalid_stat(&stat_path))?;
    let close = stat.rfind(')').ok_or_else(|| invalid_stat(&stat_path))?;
    if close <= open {
        return Err(invalid_stat(&stat_path));
    }

    let comm = &stat[open + 1..close];
    let rest = stat[close + 1..].trim();
    let fields: Vec<&str> = rest.split_whitespace().collect();

    // After removing PID and comm, fields[0] is Linux /proc stat field 3 (state).
    // Therefore: utime=14 -> 11, stime=15 -> 12, starttime=22 -> 19, rss=24 -> 21.
    if fields.len() <= 21 {
        return Err(invalid_stat(&stat_path));
    }

    let utime = parse_u64(fields[11], &stat_path)?;
    let stime = parse_u64(fields[12], &stat_path)?;
    let starttime = parse_u64(fields[19], &stat_path)?;
    let rss_pages = parse_i64(fields[21], &stat_path)?.max(0) as u64;

    let cmdline_path = format!("/proc/{pid}/cmdline");
    let name = read_cmdline_name(Path::new(&cmdline_path)).unwrap_or_else(|| comm.to_string());

    Ok(ProcessSample {
        key: ProcessKey {
            environment: EnvironmentKind::Wsl,
            pid,
            start_id: starttime,
        },
        name,
        cpu_time_secs: (utime + stime) as f64 / clock_ticks,
        memory_bytes: rss_pages.saturating_mul(page_size),
    })
}

fn read_cmdline_name(path: &Path) -> Option<String> {
    let data = fs::read(path).ok()?;
    let first = data.split(|b| *b == 0).next()?;
    if first.is_empty() {
        return None;
    }
    let full = String::from_utf8_lossy(first);
    Path::new(full.as_ref())
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .or_else(|| Some(full.into_owned()))
}

fn sysconf_positive(name: libc::c_int) -> io::Result<i64> {
    let value = unsafe { libc::sysconf(name) };
    if value <= 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(value)
    }
}

fn parse_u64(value: &str, path: &str) -> io::Result<u64> {
    value.parse::<u64>().map_err(|_| invalid_stat(path))
}

fn parse_i64(value: &str, path: &str) -> io::Result<i64> {
    value.parse::<i64>().map_err(|_| invalid_stat(path))
}

fn invalid_stat(path: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("invalid {path}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn stat_field_offsets_are_documented() {
        // This test deliberately documents the arithmetic used by read_process:
        // field 3 maps to index 0 after stripping pid and comm.
        assert_eq!(14 - 3, 11); // utime
        assert_eq!(15 - 3, 12); // stime
        assert_eq!(22 - 3, 19); // starttime
        assert_eq!(24 - 3, 21); // rss
    }
}
