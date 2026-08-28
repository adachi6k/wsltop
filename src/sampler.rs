use crate::model::{ProcessSample, ResourceKind, ResourceUsage, Snapshot};
use std::collections::HashMap;

pub fn calculate_usage(
    before: &Snapshot,
    after: &Snapshot,
    host_logical_cpu_count: u32,
) -> Vec<ResourceUsage> {
    let elapsed = after
        .captured_at
        .duration_since(before.captured_at)
        .as_secs_f64();
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

fn to_usage(sample: &ProcessSample, cpu_percent: f64) -> ResourceUsage {
    ResourceUsage {
        environment: sample.key.environment,
        source: sample.key.source.clone(),
        kind: classify_process(sample),
        id: sample.key.pid.to_string(),
        pid: Some(sample.key.pid),
        start_id: Some(sample.key.start_id),
        ppid: None,
        name: sample.name.clone(),
        args: None,
        cpu_percent,
        cpu_time_seconds: sample
            .cpu_time_secs
            .is_finite()
            .then_some(sample.cpu_time_secs.max(0.0)),
        memory_bytes: sample.memory_bytes,
    }
}

fn classify_process(sample: &ProcessSample) -> ResourceKind {
    if sample.key.environment == crate::model::EnvironmentKind::Windows
        && is_wsl_host_process(&sample.name)
    {
        ResourceKind::Host
    } else if sample.key.environment == crate::model::EnvironmentKind::Wsl
        && sample.name.eq_ignore_ascii_case("plan9")
    {
        ResourceKind::Infra
    } else {
        ResourceKind::Process
    }
}

fn is_wsl_host_process(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name == "vmmem" || name == "vmmemwsl" || name.starts_with("vmmemwslc-")
}

#[cfg(test)]
mod tests {
    use super::classify_process;
    use crate::model::{EnvironmentKind, ProcessKey, ProcessSample, ResourceKind};

    fn sample(environment: EnvironmentKind, name: &str) -> ProcessSample {
        ProcessSample {
            key: ProcessKey {
                environment,
                source: None,
                pid: 1,
                start_id: 1,
            },
            name: name.to_string(),
            cpu_time_secs: 0.0,
            memory_bytes: 0,
        }
    }

    #[test]
    fn classifies_only_wsl_plan9_as_infra() {
        assert_eq!(
            classify_process(&sample(EnvironmentKind::Wsl, "plan9")),
            ResourceKind::Infra
        );
        assert_eq!(
            classify_process(&sample(EnvironmentKind::Windows, "plan9")),
            ResourceKind::Process
        );
    }

    #[test]
    fn keeps_wsl_init_and_systemd_as_processes() {
        for name in ["init", "systemd"] {
            assert_eq!(
                classify_process(&sample(EnvironmentKind::Wsl, name)),
                ResourceKind::Process
            );
        }
    }

    #[test]
    fn classifies_windows_wsl_hosts() {
        for name in ["vmmem", "VmmemWSL", "vmmemwslc-cli-adach"] {
            assert_eq!(
                classify_process(&sample(EnvironmentKind::Windows, name)),
                ResourceKind::Host
            );
        }
    }
}
