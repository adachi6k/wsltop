//! Docker's embedded BuildKit executors are absent from `docker stats`.
//! Identify them from /proc cgroups and group their sampled processes once.
use crate::model::{ContainerProcessUsage, EnvironmentKind, ResourceKind, ResourceUsage, Snapshot};
use std::collections::BTreeMap;

/// Keep the distro in the source: PID and executor IDs are namespace-local.
pub fn source(cgroup: &str, distro: Option<&str>) -> Option<String> {
    cgroup.lines().find_map(|line| {
        let mut fields = line.splitn(3, ':');
        fields.next()?.parse::<u32>().ok()?;
        fields.next()?;
        let path = fields.next()?.strip_prefix("/docker/buildkit/")?;
        let id = path.split('/').next()?;
        valid_id(id).then(|| format!("buildkit:{}:{id}", distro.unwrap_or_default()))
    })
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn identity(source: &str) -> Option<(Option<&str>, &str)> {
    let (distro, id) = source.strip_prefix("buildkit:")?.rsplit_once(':')?;
    valid_id(id).then_some(((!distro.is_empty()).then_some(distro), id))
}

pub fn executor_id(source: &str) -> Option<&str> {
    identity(source).map(|(_, id)| id)
}

pub fn normalize_disabled_snapshot(snapshot: &mut Snapshot) {
    for process in &mut snapshot.processes {
        if process.key.environment != EnvironmentKind::Docker {
            continue;
        }
        let Some((distro, _)) = process.key.source.as_deref().and_then(identity) else {
            continue;
        };
        process.key.environment = EnvironmentKind::Wsl;
        process.key.source = distro.map(str::to_string);
    }
}

pub fn distro(row: &ResourceUsage) -> Option<&str> {
    if row.environment == EnvironmentKind::Docker {
        identity(row.source.as_deref()?).and_then(|(distro, _)| distro)
    } else {
        row.source.as_deref()
    }
}

/// Move verified executors out of WSL before constructing flat/tree/summary
/// views. These rows never enter the Docker CLI detail or exec probe queues.
pub fn take_usage(rows: &mut Vec<ResourceUsage>, enabled: bool) -> Vec<ContainerProcessUsage> {
    let mut groups = BTreeMap::<String, ContainerProcessUsage>::new();
    rows.retain_mut(|row| {
        let Some((distro, executor)) = row
            .source
            .as_deref()
            .filter(|_| row.environment == EnvironmentKind::Docker)
            .and_then(identity)
        else {
            return true;
        };
        if !enabled {
            row.environment = EnvironmentKind::Wsl;
            row.source = distro.map(str::to_string);
            return true;
        }
        let id = row.source.as_ref().unwrap().clone();
        let group = groups
            .entry(id.clone())
            .or_insert_with(|| ContainerProcessUsage {
                resource: ResourceUsage {
                    environment: EnvironmentKind::Docker,
                    source: distro.map(str::to_string),
                    kind: ResourceKind::Container,
                    id,
                    pid: None,
                    start_id: None,
                    ppid: None,
                    name: format!("BuildKit {executor}"),
                    args: None,
                    cpu_percent: 0.0,
                    cpu_time_seconds: None,
                    memory_bytes: 0,
                },
                processes: Vec::new(),
                host_pids: Vec::new(),
            });
        group.resource.cpu_percent += row.cpu_percent;
        group.resource.memory_bytes = group.resource.memory_bytes.saturating_add(row.memory_bytes);
        group.processes.push(row.clone());
        false
    });
    groups.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_explicit_buildkit_cgroups_identify_executors() {
        for cgroup in [
            "0::/docker/buildkit/executor1\n",
            "5:cpu,cpuacct:/docker/buildkit/executor1/child\n",
        ] {
            assert_eq!(
                source(cgroup, Some("docker-desktop")).as_deref(),
                Some("buildkit:docker-desktop:executor1")
            );
        }
        for cgroup in [
            "0::/",
            "0::/docker/abcdef",
            "0::/buildkit/executor1",
            "0::/docker/buildkit/",
            "0::/docker/buildkit/../other",
            "bad::/docker/buildkit/executor1",
        ] {
            assert!(source(cgroup, Some("docker-desktop")).is_none());
        }
    }

    fn process(distro: Option<&str>, id: &str) -> ResourceUsage {
        let mut row = crate::query::tests::row("git-remote-http", 2.0, 4096);
        row.environment = EnvironmentKind::Docker;
        row.source = source(&format!("0::/docker/buildkit/{id}"), distro);
        row.pid = Some(12897);
        row
    }

    #[test]
    fn groups_builds_by_distro_and_executor_without_duplicate_wsl_rows() {
        let ordinary = crate::query::tests::row("dockerd", 1.0, 1024);
        let mut rows = vec![
            ordinary.clone(),
            process(Some("docker-desktop"), "one"),
            process(Some("docker-desktop"), "one"),
            process(Some("docker-desktop"), "two"),
            process(Some("Ubuntu"), "one"),
        ];
        let groups = take_usage(&mut rows, true);
        assert_eq!(rows, [ordinary]);
        assert_eq!(groups.len(), 3);
        let group = groups
            .iter()
            .find(|g| g.resource.id == "buildkit:docker-desktop:one")
            .unwrap();
        assert_eq!(group.resource.cpu_percent, 4.0);
        assert_eq!(group.resource.memory_bytes, 8192);
        assert_eq!(group.processes.len(), 2);
        assert!(group
            .processes
            .iter()
            .all(|p| p.source.as_deref() == Some(group.resource.id.as_str())));
        assert!(group.host_pids.is_empty());
    }

    #[test]
    fn disabled_docker_keeps_processes_in_their_original_wsl_source() {
        let mut rows = vec![process(None, "one"), process(Some("docker-desktop"), "two")];
        assert!(take_usage(&mut rows, false).is_empty());
        assert!(rows.iter().all(|p| p.environment == EnvironmentKind::Wsl));
        assert_eq!(rows[0].source, None);
        assert_eq!(rows[1].source.as_deref(), Some("docker-desktop"));
    }

    #[test]
    fn disabled_docker_matches_processes_when_buildkit_cgroup_metadata_changes() {
        let captured_at = std::time::Instant::now();
        let mut before = Snapshot {
            system_cpu: None,
            captured_at,
            processes: vec![crate::model::ProcessSample {
                key: crate::model::ProcessKey {
                    environment: EnvironmentKind::Docker,
                    source: source("0::/docker/buildkit/job1", Some("docker-desktop")),
                    pid: 42,
                    start_id: 7,
                },
                name: "worker".into(),
                cpu_time_secs: 1.0,
                memory_bytes: 4096,
            }],
        };
        let mut after = Snapshot {
            system_cpu: None,
            captured_at: captured_at + std::time::Duration::from_secs(1),
            processes: vec![crate::model::ProcessSample {
                key: crate::model::ProcessKey {
                    environment: EnvironmentKind::Wsl,
                    source: Some("docker-desktop".into()),
                    pid: 42,
                    start_id: 7,
                },
                name: "worker".into(),
                cpu_time_secs: 2.0,
                memory_bytes: 8192,
            }],
        };

        normalize_disabled_snapshot(&mut before);
        normalize_disabled_snapshot(&mut after);

        let usage = crate::sampler::calculate_usage(&before, &after, 1);
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].environment, EnvironmentKind::Wsl);
        assert_eq!(usage[0].source.as_deref(), Some("docker-desktop"));
        assert_eq!(usage[0].cpu_percent, 100.0);
        assert_eq!(usage[0].memory_bytes, 8192);
    }
}
