//! Host CPU partitions measured on the host, independently of process lifetimes.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
pub struct Counter {
    pub name: String,
    pub busy: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Sample {
    Native,
    HyperV {
        physical: Vec<Counter>,
        root: Vec<Counter>,
        guest: Vec<Counter>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Breakdown {
    pub total: f64,
    pub windows: f64,
    pub virtual_machines: f64,
    pub other: f64,
}

impl Sample {
    pub fn usage_since(
        &self,
        before: &Self,
        cpus: u32,
        native_total: Option<f64>,
    ) -> Option<Breakdown> {
        if cpus == 0 {
            return None;
        }
        match (before, self) {
            (Self::Native, Self::Native) => {
                let total = native_total.filter(|v| v.is_finite() && (0.0..=100.0).contains(v))?;
                Some(Breakdown {
                    total,
                    windows: total,
                    virtual_machines: 0.0,
                    other: 0.0,
                })
            }
            (
                Self::HyperV {
                    physical: p0,
                    root: r0,
                    guest: g0,
                },
                Self::HyperV {
                    physical,
                    root,
                    guest,
                },
            ) => {
                // Physical/root instance counts must describe the entire host.
                if physical.len() != cpus as usize || root.len() != cpus as usize {
                    return None;
                }
                let total = usage(p0, physical, cpus)?;
                let windows = usage(r0, root, cpus)?;
                let virtual_machines = usage(g0, guest, cpus)?;
                let other = total - windows - virtual_machines;
                // Sequential provider reads may disagree at a transition. Don't
                // hide an overcount by clamping or rescaling partition values.
                if other < 0.0 || total > 100.0 {
                    return None;
                }
                Some(Breakdown {
                    total,
                    windows,
                    virtual_machines,
                    other,
                })
            }
            _ => None,
        }
    }
}

fn usage(before: &[Counter], after: &[Counter], cpus: u32) -> Option<f64> {
    let old: BTreeMap<_, _> = before.iter().map(|p| (&p.name, p)).collect();
    let new: BTreeMap<_, _> = after.iter().map(|p| (&p.name, p)).collect();
    if old.len() != before.len() || new.len() != after.len() || !old.keys().eq(new.keys()) {
        return None;
    }
    let mut sum = 0.0;
    for (name, p) in new {
        let q = old[name];
        let elapsed = p.total.checked_sub(q.total)?;
        let busy = p.busy.checked_sub(q.busy)?;
        if elapsed == 0 || busy > elapsed {
            return None;
        }
        sum += 100.0 * busy as f64 / elapsed as f64 / cpus as f64;
    }
    Some(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn counters(prefix: &str, n: usize, busy: u64, total: u64) -> Vec<Counter> {
        (0..n)
            .map(|i| Counter {
                name: format!("{prefix}{i}"),
                busy,
                total,
            })
            .collect()
    }
    fn sample(busy: u64, total: u64) -> Sample {
        Sample::HyperV {
            physical: counters("lp", 2, busy, total),
            root: counters("root", 2, busy / 2, total),
            guest: counters("guest", 4, busy / 8, total),
        }
    }
    #[test]
    fn guest_vcpus_use_host_denominator_and_include_all_work() {
        let result = sample(80, 100).usage_since(&sample(0, 0), 2, None).unwrap();
        assert_eq!(
            result,
            Breakdown {
                total: 80.0,
                windows: 40.0,
                virtual_machines: 20.0,
                other: 20.0
            }
        );
        // No process rows participate: exited tasks and interrupts remain counted.
    }
    #[test]
    fn topology_reset_missing_instances_and_overcount_are_unavailable() {
        assert!(sample(80, 100)
            .usage_since(&sample(100, 200), 2, None)
            .is_none());
        assert!(sample(80, 100)
            .usage_since(&sample(0, 0), 4, None)
            .is_none());
        let mut after = sample(80, 100);
        if let Sample::HyperV { guest, .. } = &mut after {
            guest.pop();
        }
        assert!(after.usage_since(&sample(0, 0), 2, None).is_none());
        let mut after = sample(80, 100);
        if let Sample::HyperV { root, .. } = &mut after {
            root.iter_mut().for_each(|p| p.busy = 80);
        }
        assert!(after.usage_since(&sample(0, 0), 2, None).is_none());
        assert!(Sample::Native
            .usage_since(&sample(0, 0), 2, Some(30.0))
            .is_none());
    }
    #[test]
    fn native_host_keeps_interrupts_and_exited_work_in_windows_total() {
        let b = Sample::Native
            .usage_since(&Sample::Native, 128, Some(35.1))
            .unwrap();
        assert_eq!(
            b,
            Breakdown {
                total: 35.1,
                windows: 35.1,
                virtual_machines: 0.0,
                other: 0.0,
            }
        );
        assert!(Sample::Native
            .usage_since(&Sample::Native, 16, None)
            .is_none());
        assert!(Sample::Native
            .usage_since(&Sample::Native, 16, Some(f64::NAN))
            .is_none());
    }

    #[test]
    fn duplicate_counters_and_missing_guest_baselines_are_not_zero_usage() {
        let mut after = sample(80, 100);
        if let Sample::HyperV { physical, .. } = &mut after {
            physical[1].name = physical[0].name.clone();
        }
        assert!(after.usage_since(&sample(0, 0), 2, None).is_none());
        let mut before = sample(0, 0);
        if let Sample::HyperV { guest, .. } = &mut before {
            guest.clear();
        }
        assert!(sample(80, 100).usage_since(&before, 2, None).is_none());
    }
}
