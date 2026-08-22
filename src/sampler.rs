use crate::model::{ProcessSample, ProcessUsage, Snapshot};
use std::collections::HashMap;

pub fn calculate_usage(
    before: &Snapshot,
    after: &Snapshot,
    host_logical_cpu_count: u32,
) -> Vec<ProcessUsage> {
    let elapsed = after.captured_at.duration_since(before.captured_at).as_secs_f64();
    if elapsed <= 0.0 || host_logical_cpu_count == 0 {
        return Vec::new();
    }

    let previous: HashMap<_, _> = before
        .processes
        .iter()
        .map(|p| (p.key.clone(), p))
        .collect();

    let denominator = elapsed * host_logical_cpu_count as f64;
    let mut result = Vec::new();

    for current in &after.processes {
        let Some(old) = previous.get(&current.key) else {
            continue;
        };

        let delta = current.cpu_time_secs - old.cpu_time_secs;
        if !delta.is_finite() || delta < 0.0 {
            // Negative values normally mean PID reuse or a collector reset.
            continue;
        }

        let cpu_percent = delta / denominator * 100.0;
        if !cpu_percent.is_finite() {
            continue;
        }

        result.push(to_usage(current, cpu_percent.max(0.0)));
    }

    result
}

fn to_usage(sample: &ProcessSample, cpu_percent: f64) -> ProcessUsage {
    ProcessUsage {
        environment: sample.key.environment,
        pid: sample.key.pid,
        name: sample.name.clone(),
        cpu_percent,
        memory_bytes: sample.memory_bytes,
    }
}
