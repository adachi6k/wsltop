//! Exclusive guest CPU estimates over a common, bracketed kernel-time window.
use crate::{command, linux_cpu};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, Instant};

// A read-only probe in an already running container. Restrict support to a leaf
// cgroup v2 namespace rooted at the container; never read a host-wide root or
// subtract overlapping parent/child cgroups. Missing tools/permissions fall back.
const PROBE: &str = r#"set -eu
[ "$(cat /proc/self/cgroup)" = '0::/' ]
[ "$(stat -f -c %T /sys/fs/cgroup)" = cgroup2fs ]
[ -r /sys/fs/cgroup/cpu.max ]
awk '$1 == "nr_descendants" { found=1; if ($2 != 0) exit 1 } END { if (!found) exit 1 }' /sys/fs/cgroup/cgroup.stat
read begin rest < /proc/uptime
printf 'BOOT '; cat /proc/sys/kernel/random/boot_id
printf 'GROUP '; stat -Lc '%d:%i' /sys/fs/cgroup
awk '$1 == "usage_usec" { print "USAGE", $2 }' /sys/fs/cgroup/cpu.stat
read end rest < /proc/uptime
printf 'TIME %s %s\n' "$begin" "$end"
"#;

#[derive(Debug, Clone)]
pub struct Probe {
    pub id: String,
    sample: Option<Point>,
}

#[derive(Debug, Clone)]
struct Point {
    identity: String,
    boot: String,
    time: f64,
    seconds: f64,
    received: Instant,
}

/// Probe a bounded number concurrently. No container is started or modified.
pub fn collect(program: &str, ids: &[&str]) -> Vec<Probe> {
    if ids.len() > 16 {
        return ids
            .iter()
            .map(|id| Probe {
                id: (*id).into(),
                sample: None,
            })
            .collect();
    }
    let mut result = Vec::new();
    for chunk in ids.chunks(4) {
        result.extend(std::thread::scope(|scope| {
            let jobs: Vec<_> = chunk
                .iter()
                .map(|&id| {
                    scope.spawn(move || {
                        if !valid_id(id) {
                            return Probe {
                                id: id.into(),
                                sample: None,
                            };
                        }
                        let sample = command::output_with_timeout(
                            command::CommandSpec::new(program, &["exec", id, "sh", "-c", PROBE]),
                            Duration::from_secs(2),
                        )
                        .ok()
                        .filter(|o| o.status.success())
                        .and_then(|o| parse(&String::from_utf8(o.stdout).ok()?));
                        Probe {
                            id: id.into(),
                            sample,
                        }
                    })
                })
                .collect();
            jobs.into_iter()
                .zip(chunk)
                .map(|(job, &id)| {
                    job.join().unwrap_or(Probe {
                        id: id.into(),
                        sample: None,
                    })
                })
                .collect::<Vec<_>>()
        }));
    }
    result
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    const BOOT: &str = "8ac95ec1-d8c0-4372-b478-8b113f4bd600";

    fn probe(id: &str, group: &str, time: f64, rate: f64) -> Probe {
        Probe {
            id: id.into(),
            sample: Some(Point {
                identity: group.into(),
                boot: BOOT.into(),
                time,
                seconds: time * rate,
                received: Instant::now(),
            }),
        }
    }

    fn history() -> History {
        let mut h = History::default();
        for t in [10.0, 13.0, 16.0, 19.0] {
            let kernel = linux_cpu::Sample::parse(
                &format!("cpu {} 0 0 10000 0 0 0\ncpu0 0\ncpu1 0", t * 100.0),
                &t.to_string(),
                BOOT,
                100.0,
            )
            .unwrap();
            h.kernel(Some(&kernel));
        }
        for t in [10.2, 13.2, 16.2] {
            h.containers(3, &[probe("docker", "25:10", t, 0.2)]);
            h.containers(2, &[probe("wslc", "25:11", t + 0.1, 0.1)]);
        }
        h
    }

    #[test]
    fn subtracts_cgroup_deltas_in_one_bracketed_window() {
        let h = history();
        let r = h
            .exclusive(&[(2, "wslc"), (3, "docker")], 2, Duration::from_secs(3))
            .unwrap();
        for (value, expected) in [(r.wsl, 35.0), (r.docker, 10.0), (r.wslc, 5.0)] {
            assert!((value - expected).abs() < 1e-9);
        }
        // CLI rates are not inputs; no scaling to a Windows host total occurs.
        assert!((r.wsl + r.docker + r.wslc - 50.0).abs() < 1e-9);
    }

    #[test]
    fn exact_cgroup_alias_is_counted_once_and_owned_by_docker() {
        let mut h = history();
        let docker = h.containers[&(3, "docker".into())].clone();
        h.containers.insert((2, "alias".into()), docker);
        let r = h
            .exclusive(&[(2, "alias"), (3, "docker")], 2, Duration::from_secs(3))
            .unwrap();
        assert!((r.wsl - 40.0).abs() < 1e-9);
        assert_eq!(r.wslc, 0.0);
        assert!((r.docker - 10.0).abs() < 1e-9);
    }

    #[test]
    fn rejects_foreign_kernel_stale_missing_and_negative_residual() {
        for case in 0..5 {
            let mut h = history();
            let s = h.containers.get_mut(&(3, "docker".into())).unwrap();
            match case {
                0 => s.0.back_mut().unwrap().boot = "other-boot".into(),
                1 => s.0.back_mut().unwrap().received = Instant::now() - Duration::from_secs(60),
                2 => s.0.clear(),
                3 => {
                    for p in &mut s.0 {
                        p.seconds = p.time * 10.0;
                    }
                }
                _ => {
                    for p in &mut s.0 {
                        p.time += 1000.0;
                    }
                }
            }
            assert!(
                h.exclusive(&[(3, "docker")], 2, Duration::from_secs(3))
                    .is_err(),
                "case {case}"
            );
        }
    }

    #[test]
    fn restart_reset_failure_and_disappearance_discard_baselines() {
        for case in 0..4 {
            let mut h = history();
            let mut p = probe("docker", "25:10", 19.2, 0.2);
            match case {
                0 => p.sample.as_mut().unwrap().seconds = 0.0,
                1 => p.sample.as_mut().unwrap().identity = "25:99".into(),
                2 => p.sample.as_mut().unwrap().boot = "restart".into(),
                _ => p.sample = None,
            }
            h.containers(3, &[p]);
            assert!(h
                .exclusive(&[(3, "docker")], 2, Duration::from_secs(3))
                .is_err());
        }
        let mut h = history();
        h.containers(3, &[]);
        assert!(!h.containers.contains_key(&(3, "docker".into())));
        h.kernel(None);
        assert!(h.kernel.0.is_empty());
    }

    #[test]
    fn parses_only_complete_bounded_cgroup_reads() {
        let good = format!("BOOT {BOOT}\nGROUP 25:10\nUSAGE 1500000\nTIME 10.00 10.02\n");
        let p = parse(&good).unwrap();
        assert_eq!(p.seconds, 1.5);
        assert!((p.time - 10.01).abs() < 1e-9);
        for bad in [
            good.replace("10.02", "11.00"),
            good.replace("10.02", "NaN"),
            good.replace("1500000", "-1"),
            good.replace("25:10", "bad"),
            format!("{good}USAGE 3\n"),
            good.replace("BOOT ", "BAD "),
        ] {
            assert!(parse(&bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn fallback_keeps_inclusive_values_and_memory_and_marks_overlap() {
        let mut snapshot = crate::snapshot_store::tests::snapshot(60.0);
        snapshot.environment_summary = crate::summary::EnvironmentSummary(
            [Some(crate::summary::Usage {
                cpu_percent: Some(12.0),
                memory_bytes: Some(1234),
            }); 4],
        );
        apply(
            &mut snapshot,
            &History::default(),
            &[(3, "docker")],
            true,
            Duration::from_secs(3),
        );
        assert!(snapshot.cpu_overlap_unresolved);
        assert_eq!(
            snapshot.environment_summary.0[1].unwrap().cpu_percent,
            Some(12.0)
        );
        assert_eq!(
            snapshot.environment_summary.0[1].unwrap().memory_bytes,
            Some(1234)
        );
        assert!(snapshot
            .warnings
            .iter()
            .any(|w| w.contains("overlap unresolved")));
    }
}

fn parse(text: &str) -> Option<Point> {
    let mut fields = BTreeMap::new();
    for line in text.lines() {
        let (key, value) = line.split_once(' ')?;
        if fields.insert(key, value.trim()).is_some() {
            return None;
        }
    }
    let boot = *fields.get("BOOT")?;
    if boot.len() != 36 || !boot.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') {
        return None;
    }
    let identity = *fields.get("GROUP")?;
    let (device, inode) = identity.split_once(':')?;
    device.parse::<u64>().ok()?;
    inode.parse::<u64>().ok()?;
    let seconds = fields.get("USAGE")?.parse::<u64>().ok()? as f64 / 1_000_000.0;
    let times = fields
        .get("TIME")?
        .split_whitespace()
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    if times.len() != 2
        || times.iter().any(|t| !t.is_finite() || *t < 0.0)
        || times[1] < times[0]
        || times[1] - times[0] > 0.1
    {
        return None;
    }
    Some(Point {
        identity: identity.into(),
        boot: boot.into(),
        time: (times[0] + times[1]) / 2.0,
        seconds,
        received: Instant::now(),
    })
}

#[derive(Debug, Clone, Default)]
struct Series(VecDeque<Point>);

impl Series {
    fn push(&mut self, point: Point) {
        if self.0.back().is_some_and(|old| {
            old.boot != point.boot
                || old.identity != point.identity
                || point.time <= old.time
                || point.seconds < old.seconds
        }) {
            self.0.clear();
        }
        self.0.push_back(point);
        while self.0.len() > 8 {
            self.0.pop_front();
        }
    }

    fn at(&self, time: f64, max_gap: f64) -> Option<f64> {
        for (a, b) in self.0.iter().zip(self.0.iter().skip(1)) {
            if a.time <= time && time <= b.time && b.time - a.time <= max_gap {
                return Some(
                    a.seconds + (b.seconds - a.seconds) * (time - a.time) / (b.time - a.time),
                );
            }
        }
        None
    }
}

#[derive(Debug, Clone, Default)]
pub struct History {
    kernel: Series,
    containers: BTreeMap<(usize, String), Series>,
}

#[derive(Debug, Clone, Copy)]
pub struct Exclusive {
    pub wsl: f64,
    pub wslc: f64,
    pub docker: f64,
}

pub fn apply(
    snapshot: &mut crate::monitor::MonitorSnapshot,
    history: &History,
    targets: &[(usize, &str)],
    ready: bool,
    interval: Duration,
) {
    if targets.is_empty() && ready {
        return;
    }
    let complete = snapshot.environment_summary.0[1]
        .and_then(|u| u.cpu_percent)
        .is_some()
        && targets.iter().all(|(env, _)| {
            snapshot.environment_summary.0[*env]
                .and_then(|u| u.cpu_percent)
                .is_some()
        });
    let result = if ready && complete {
        history.exclusive(targets, snapshot.host_logical_cpu_count, interval)
    } else {
        Err("container collection unavailable or warming up")
    };
    match result {
        Ok(cpu) => {
            for (index, value) in [(1, cpu.wsl), (2, cpu.wslc), (3, cpu.docker)] {
                if let Some(usage) = &mut snapshot.environment_summary.0[index] {
                    usage.cpu_percent = Some(value);
                }
            }
        }
        Err(reason) => {
            snapshot.cpu_overlap_unresolved = true;
            snapshot.warnings.push(format!(
                "WSL* CPU overlap unresolved: {reason}; showing inclusive observations"
            ));
        }
    }
}

impl History {
    pub fn kernel(&mut self, sample: Option<&linux_cpu::Sample>) {
        let Some(sample) = sample else {
            self.kernel.0.clear();
            return;
        };
        let (boot, time, seconds, cpus, ticks) = sample.cumulative();
        self.kernel.push(Point {
            boot: boot.into(),
            time,
            seconds,
            identity: format!("{cpus}:{ticks}"),
            received: Instant::now(),
        });
    }

    pub fn containers(&mut self, environment: usize, probes: &[Probe]) {
        let ids: BTreeSet<_> = probes.iter().map(|p| p.id.as_str()).collect();
        self.containers
            .retain(|(env, id), _| *env != environment || ids.contains(id.as_str()));
        for probe in probes {
            let key = (environment, probe.id.clone());
            if let Some(point) = &probe.sample {
                self.containers.entry(key).or_default().push(point.clone());
            } else {
                self.containers.remove(&key);
            }
        }
    }

    pub fn exclusive(
        &self,
        targets: &[(usize, &str)],
        cpus: u32,
        interval: Duration,
    ) -> Result<Exclusive, &'static str> {
        if targets.is_empty() {
            return Err("no containers");
        }
        let kernel = self
            .kernel
            .0
            .back()
            .ok_or("WSL kernel counters unavailable")?;
        let window = interval.as_secs_f64().max(2.0);
        let max_age = Duration::from_secs_f64(window * 2.0 + 1.0);
        let mut sources = Vec::new();
        let mut identities = BTreeSet::new();
        // Docker owns an exact cgroup alias also exposed by WSLC.
        let mut targets = targets.to_vec();
        targets.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        for (env, id) in targets {
            let series = self
                .containers
                .get(&(env, id.into()))
                .ok_or("container cgroup counters unavailable")?;
            let point = series.0.back().ok_or("container counters warming up")?;
            if point.boot != kernel.boot {
                return Err("container belongs to another kernel");
            }
            if point.received.elapsed() > max_age {
                return Err("container counters stale");
            }
            if identities.insert(point.identity.as_str()) {
                sources.push((env, series));
            }
        }
        if kernel.received.elapsed() > max_age || cpus == 0 {
            return Err("WSL counters stale or unnormalized");
        }
        let end = sources
            .iter()
            .map(|(_, s)| s.0.back().unwrap().time)
            .fold(kernel.time, f64::min);
        let start = end - window;
        let delta = |s: &Series| -> Result<f64, &'static str> {
            let a = s
                .at(start, window * 2.0)
                .ok_or("common CPU window warming up or incomplete")?;
            let b = s
                .at(end, window * 2.0)
                .ok_or("common CPU window warming up or incomplete")?;
            if b < a {
                return Err("CPU counter reset");
            }
            Ok((b - a) / window / cpus as f64 * 100.0)
        };
        let mut result = Exclusive {
            wsl: delta(&self.kernel)?,
            wslc: 0.0,
            docker: 0.0,
        };
        for (env, series) in sources {
            let cpu = delta(series)?;
            match env {
                2 => result.wslc += cpu,
                3 => result.docker += cpu,
                _ => return Err("unknown container environment"),
            }
        }
        result.wsl -= result.wslc + result.docker;
        if !result.wsl.is_finite() || result.wsl < 0.0 {
            return Err("container CPU exceeds WSL CPU in common window");
        }
        Ok(result)
    }
}
