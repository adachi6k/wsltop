//! WSL kernel-wide CPU observation, sampled once from the primary distribution.
//! /proc/stat is shared by distributions; never add one value per distribution.

#[derive(Debug, Clone)]
pub struct Sample {
    busy: [u64; 5],
    uptime: f64,
    ticks: f64,
    cpus: usize,
    boot: String,
}

impl Sample {
    pub fn parse(stat: &str, uptime: &str, boot: &str, ticks: f64) -> Option<Self> {
        let fields: Vec<_> = stat
            .lines()
            .find(|line| line.starts_with("cpu "))?
            .split_whitespace()
            .skip(1)
            .collect();
        let busy = [0, 1, 2, 5, 6].map(|index| fields.get(index)?.parse::<u64>().ok());
        let busy = busy
            .into_iter()
            .collect::<Option<Vec<_>>>()?
            .try_into()
            .ok()?;
        let uptime = uptime.split_whitespace().next()?.parse::<f64>().ok()?;
        let cpus = stat
            .lines()
            .filter(|line| {
                line.strip_prefix("cpu")
                    .is_some_and(|s| s.starts_with(|c: char| c.is_ascii_digit()))
            })
            .count();
        let boot = boot.trim();
        if !uptime.is_finite()
            || uptime < 0.0
            || !ticks.is_finite()
            || ticks <= 0.0
            || cpus == 0
            || boot.is_empty()
        {
            return None;
        }
        Some(Self {
            busy,
            uptime,
            ticks,
            cpus,
            boot: boot.into(),
        })
    }

    pub fn usage_since(&self, before: &Self, host_cpus: u32) -> Option<f64> {
        if self.boot != before.boot
            || self.cpus != before.cpus
            || self.ticks != before.ticks
            || host_cpus == 0
        {
            return None;
        }
        let elapsed = self.uptime - before.uptime;
        if elapsed <= 0.0 {
            return None;
        }
        let busy = self
            .busy
            .iter()
            .zip(before.busy)
            .try_fold(0u64, |sum, (after, before)| {
                sum.checked_add(after.checked_sub(before)?)
            })?;
        // user/nice already include guest time. Idle, iowait and steal are not
        // executed CPU work. Normalize elapsed CPU time by Windows host cores.
        Some(busy as f64 / self.ticks / elapsed / host_cpus as f64 * 100.0)
    }
}

pub fn usage(
    before: &crate::model::Snapshot,
    after: &crate::model::Snapshot,
    cpus: u32,
) -> Option<f64> {
    after
        .system_cpu
        .as_ref()?
        .usage_since(before.system_cpu.as_ref()?, cpus)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sample(cpu: &str, uptime: &str) -> Sample {
        Sample::parse(
            &format!("cpu {cpu}\ncpu0 0\ncpu1 0\n"),
            uptime,
            "boot-a",
            100.0,
        )
        .unwrap()
    }

    #[test]
    fn counts_kernel_work_and_churn_without_double_counting_guest_or_wait() {
        let before = sample("100 20 30 400 50 6 7 80 90 10", "10");
        let after = sample("200 40 60 500 90 16 17 200 180 30", "11");
        // 100 user + 20 nice + 30 system + 10 irq + 10 softirq = 170 ticks.
        assert_eq!(after.usage_since(&before, 2), Some(85.0));
        assert_eq!(after.usage_since(&before, 4), Some(42.5));
        let old = crate::model::Snapshot {
            system_cpu: Some(before),
            captured_at: std::time::Instant::now(),
            processes: vec![],
        };
        let new = crate::model::Snapshot {
            system_cpu: Some(after),
            captured_at: old.captured_at + std::time::Duration::from_secs(1),
            processes: vec![],
        };
        assert!(crate::sampler::calculate_usage(&old, &new, 2).is_empty());
        assert_eq!(usage(&old, &new, 2), Some(85.0));
    }

    #[test]
    fn rejects_reset_restart_topology_and_invalid_time() {
        let old = sample("100 20 30 400 50 6 7", "10");
        let mut new = sample("200 40 60 500 90 16 17", "11");
        assert!(new.usage_since(&old, 0).is_none());
        new.busy[1] = 0; // Reject individual reset even if the total grew.
        assert!(new.usage_since(&old, 2).is_none());
        new = old.clone();
        assert!(new.usage_since(&old, 2).is_none());
        new.uptime = 9.0;
        assert!(new.usage_since(&old, 2).is_none());
        new.uptime = 11.0;
        new.boot = "boot-b".into();
        assert!(new.usage_since(&old, 2).is_none());
        new.boot.clone_from(&old.boot);
        new.cpus = 4;
        assert!(new.usage_since(&old, 2).is_none());
        new.cpus = 2;
        new.ticks = 1000.0;
        assert!(new.usage_since(&old, 2).is_none());
    }

    #[test]
    fn rejects_missing_malformed_and_nonfinite_counters() {
        for stat in ["", "cpu 1 2\ncpu0 0", "cpu 1 2 x 4 5 6 7\ncpu0 0"] {
            assert!(Sample::parse(stat, "10", "boot", 100.0).is_none());
        }
        let stat = "cpu 1 2 3 4 5 6 7\ncpu0 0";
        for time in ["NaN", "inf", "-1", ""] {
            assert!(Sample::parse(stat, time, "boot", 100.0).is_none());
        }
        for ticks in [0.0, f64::NAN, f64::INFINITY] {
            assert!(Sample::parse(stat, "10", "boot", ticks).is_none());
        }
        assert!(Sample::parse(stat, "10", "", 100.0).is_none());
    }
}
