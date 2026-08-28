use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage, WindowsApplicationUsage};
use serde::Serialize;
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingStatus {
    Resolved,
    Unresolved,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributionGroup {
    pub name: String,
    pub host: ResourceUsage,
    pub cpu_percent: f64,
    pub children: Vec<ResourceUsage>,
    pub known_children_cpu_percent: f64,
    pub unattributed_cpu_percent: f64,
    pub over_attributed_cpu_percent: f64,
    pub mapping_status: MappingStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttributionTree {
    pub host_logical_cpu_count: u32,
    pub groups: Vec<AttributionGroup>,
    /// Children retained here when no unique host mapping can be made.
    pub unmapped_children: Vec<ResourceUsage>,
    pub docker_groups: Vec<DockerAttributionGroup>,
    pub wslc_groups: Vec<DockerAttributionGroup>,
    pub windows_applications: Vec<WindowsApplicationUsage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DockerAttributionGroup {
    pub container: ResourceUsage,
    pub children: Vec<ResourceUsage>,
    pub unattributed_cpu_percent: f64,
    pub over_attributed_cpu_percent: f64,
}

pub fn build_tree_with_docker(
    host_logical_cpu_count: u32,
    hosts: &[ResourceUsage],
    wsl_children: &[ResourceUsage],
    wslc_children: &[ResourceUsage],
    docker: &[crate::model::ContainerProcessUsage],
) -> AttributionTree {
    let mut docker_pids = std::collections::HashSet::new();
    for container in docker {
        docker_pids.extend(container.host_pids.iter().copied());
    }
    let direct_wsl: Vec<_> = wsl_children
        .iter()
        .filter(|row| {
            !is_current_wsl_process(row) || row.pid.is_none_or(|pid| !docker_pids.contains(&pid))
        })
        .cloned()
        .collect();
    let mut docker_groups = Vec::new();
    for container in docker {
        let mut children = if container.host_pids.is_empty() {
            container.processes.clone()
        } else {
            wsl_children
                .iter()
                .filter(|row| {
                    is_current_wsl_process(row)
                        && row
                            .pid
                            .is_some_and(|pid| container.host_pids.contains(&pid))
                })
                .cloned()
                .collect()
        };
        children.sort_by(|left, right| {
            right
                .cpu_percent
                .partial_cmp(&left.cpu_percent)
                .unwrap_or(Ordering::Equal)
        });
        let known: f64 = children.iter().map(|row| row.cpu_percent).sum();
        let difference = container.resource.cpu_percent - known;
        docker_groups.push(DockerAttributionGroup {
            container: container.resource.clone(),
            children,
            unattributed_cpu_percent: difference.max(0.0),
            over_attributed_cpu_percent: (-difference).max(0.0),
        });
    }
    let mut wsl_parent_children = direct_wsl;
    wsl_parent_children.extend(
        docker
            .iter()
            .filter(|item| !item.host_pids.is_empty())
            .map(|item| item.resource.clone()),
    );
    let wsl_hosts: Vec<_> = hosts
        .iter()
        .filter(|row| is_wsl_host(&row.name))
        .cloned()
        .collect();
    let wslc_hosts: Vec<_> = hosts
        .iter()
        .filter(|row| is_wslc_host(&row.name))
        .cloned()
        .collect();

    let mut groups = Vec::new();
    let mut unmapped_children = Vec::new();

    add_groups(
        &mut groups,
        &mut unmapped_children,
        wsl_hosts,
        &wsl_parent_children,
        |_| "WSL VM".to_string(),
    );
    add_groups(
        &mut groups,
        &mut unmapped_children,
        wslc_hosts,
        wslc_children,
        |host| {
            let session = host
                .name
                .strip_prefix("vmmemwslc-")
                .or_else(|| host.name.strip_prefix("VmmemWSLC-"))
                .unwrap_or(&host.name);
            format!("WSLC session: {session}")
        },
    );

    AttributionTree {
        host_logical_cpu_count,
        groups,
        unmapped_children,
        docker_groups,
        wslc_groups: Vec::new(),
        windows_applications: Vec::new(),
    }
}

pub fn attach_wslc_processes(
    tree: &mut AttributionTree,
    containers: &[crate::model::ContainerProcessUsage],
) {
    tree.wslc_groups = containers
        .iter()
        .map(|container| {
            let mut children = container.processes.clone();
            children.sort_by(|left, right| {
                right
                    .cpu_percent
                    .partial_cmp(&left.cpu_percent)
                    .unwrap_or(Ordering::Equal)
            });
            let known: f64 = children.iter().map(|row| row.cpu_percent).sum();
            let difference = container.resource.cpu_percent - known;
            DockerAttributionGroup {
                container: container.resource.clone(),
                children,
                unattributed_cpu_percent: difference.max(0.0),
                over_attributed_cpu_percent: (-difference).max(0.0),
            }
        })
        .collect();
}

fn is_current_wsl_process(resource: &ResourceUsage) -> bool {
    resource.environment == EnvironmentKind::Wsl && resource.source.is_none()
}

fn add_groups<F>(
    groups: &mut Vec<AttributionGroup>,
    unmapped_children: &mut Vec<ResourceUsage>,
    hosts: Vec<ResourceUsage>,
    children: &[ResourceUsage],
    name: F,
) where
    F: Fn(&ResourceUsage) -> String,
{
    let resolved = hosts.len() == 1;
    if !resolved {
        unmapped_children.extend(children.iter().cloned());
    }

    for host in hosts {
        let mapped_children = if resolved {
            children.to_vec()
        } else {
            Vec::new()
        };
        groups.push(make_group(
            name(&host),
            host,
            mapped_children,
            if resolved {
                MappingStatus::Resolved
            } else {
                MappingStatus::Unresolved
            },
        ));
    }
}

fn make_group(
    name: String,
    host: ResourceUsage,
    mut children: Vec<ResourceUsage>,
    mapping_status: MappingStatus,
) -> AttributionGroup {
    children.sort_by(|left, right| {
        right
            .cpu_percent
            .partial_cmp(&left.cpu_percent)
            .unwrap_or(Ordering::Equal)
    });
    let known_children_cpu_percent = children.iter().map(|row| row.cpu_percent).sum::<f64>();
    let difference = host.cpu_percent - known_children_cpu_percent;

    AttributionGroup {
        name,
        cpu_percent: host.cpu_percent,
        host,
        children,
        known_children_cpu_percent,
        unattributed_cpu_percent: difference.max(0.0),
        over_attributed_cpu_percent: (-difference).max(0.0),
        mapping_status,
    }
}

fn is_wsl_host(name: &str) -> bool {
    name.eq_ignore_ascii_case("vmmem") || name.eq_ignore_ascii_case("vmmemwsl")
}

fn is_wslc_host(name: &str) -> bool {
    name.to_ascii_lowercase().starts_with("vmmemwslc-")
}

pub fn is_host_resource(row: &ResourceUsage) -> bool {
    row.environment == EnvironmentKind::Windows && row.kind == ResourceKind::Host
}

pub fn hide_infra(tree: &mut AttributionTree) {
    for group in &mut tree.groups {
        group
            .children
            .retain(|child| child.kind != ResourceKind::Infra);
    }
    tree.unmapped_children
        .retain(|child| child.kind != ResourceKind::Infra);
    for docker in &mut tree.docker_groups {
        docker
            .children
            .retain(|child| child.kind != ResourceKind::Infra);
    }
    for wslc in &mut tree.wslc_groups {
        wslc.children
            .retain(|child| child.kind != ResourceKind::Infra);
    }
}

#[cfg(test)]
mod tests {
    use super::{build_tree_with_docker, make_group, MappingStatus};
    use crate::model::{ContainerProcessUsage, EnvironmentKind, ResourceKind, ResourceUsage};

    fn resource(kind: ResourceKind, name: &str, cpu_percent: f64) -> ResourceUsage {
        ResourceUsage {
            environment: EnvironmentKind::Wsl,
            source: None,
            kind,
            id: name.to_string(),
            pid: Some(1),
            start_id: None,
            ppid: None,
            name: name.to_string(),
            args: None,
            cpu_percent,
            cpu_time_seconds: None,
            memory_bytes: 0,
        }
    }

    fn windows_host(name: &str, cpu_percent: f64) -> ResourceUsage {
        let mut host = resource(ResourceKind::Host, name, cpu_percent);
        host.environment = EnvironmentKind::Windows;
        host
    }

    fn group(host_cpu: f64, children_cpu: &[f64]) -> super::AttributionGroup {
        make_group(
            "test".to_string(),
            resource(ResourceKind::Host, "host", host_cpu),
            children_cpu
                .iter()
                .enumerate()
                .map(|(index, cpu)| resource(ResourceKind::Process, &index.to_string(), *cpu))
                .collect(),
            MappingStatus::Resolved,
        )
    }

    #[test]
    fn calculates_unattributed_cpu() {
        let group = group(10.0, &[7.0]);
        assert_eq!(group.unattributed_cpu_percent, 3.0);
        assert_eq!(group.over_attributed_cpu_percent, 0.0);
    }

    #[test]
    fn zeroes_unattributed_when_fully_attributed() {
        assert_eq!(group(10.0, &[10.0]).unattributed_cpu_percent, 0.0);
    }

    #[test]
    fn clamps_unattributed_and_records_sampling_skew() {
        let group = group(10.0, &[12.0]);
        assert_eq!(group.unattributed_cpu_percent, 0.0);
        assert_eq!(group.over_attributed_cpu_percent, 2.0);
        assert_eq!(group.children[0].cpu_percent, 12.0);
    }

    #[test]
    fn maps_default_wslc_children_to_one_session_host() {
        let container = resource(ResourceKind::Container, "demo", 4.0);
        let tree = build_tree_with_docker(
            16,
            &[windows_host("vmmemwslc-cli-user", 10.0)],
            &[],
            &[container],
            &[],
        );

        assert_eq!(tree.groups.len(), 1);
        assert_eq!(tree.groups[0].mapping_status, MappingStatus::Resolved);
        assert_eq!(tree.groups[0].children[0].kind, ResourceKind::Container);
        assert!(tree.unmapped_children.is_empty());
    }

    #[test]
    fn does_not_guess_between_multiple_wslc_hosts() {
        let container = resource(ResourceKind::Container, "demo", 4.0);
        let tree = build_tree_with_docker(
            16,
            &[
                windows_host("vmmemwslc-cli-a", 10.0),
                windows_host("vmmemwslc-cli-b", 10.0),
            ],
            &[],
            &[container],
            &[],
        );

        assert_eq!(tree.groups.len(), 2);
        assert!(tree
            .groups
            .iter()
            .all(|group| group.mapping_status == MappingStatus::Unresolved
                && group.children.is_empty()));
        assert_eq!(tree.unmapped_children.len(), 1);
    }

    #[test]
    fn nests_docker_processes_without_double_counting_wsl_children() {
        let mut process = resource(ResourceKind::Process, "nginx", 2.0);
        process.pid = Some(42);
        let container = ContainerProcessUsage {
            resource: {
                let mut row = resource(ResourceKind::Container, "web", 3.0);
                row.environment = EnvironmentKind::Docker;
                row
            },
            processes: Vec::new(),
            host_pids: vec![42],
        };
        let tree = build_tree_with_docker(
            16,
            &[windows_host("vmmemwsl", 10.0)],
            &[process],
            &[],
            &[container],
        );
        assert_eq!(tree.groups[0].children.len(), 1);
        assert_eq!(tree.groups[0].children[0].kind, ResourceKind::Container);
        assert_eq!(tree.docker_groups[0].children[0].pid, Some(42));
        assert_eq!(tree.docker_groups[0].unattributed_cpu_percent, 1.0);
        assert_eq!(tree.groups[0].unattributed_cpu_percent, 7.0);
    }

    #[test]
    fn docker_pid_matching_does_not_cross_wsl_distribution_sources() {
        let mut current = resource(ResourceKind::Process, "current", 2.0);
        current.pid = Some(42);
        let mut additional = resource(ResourceKind::Process, "additional", 1.0);
        additional.pid = Some(42);
        additional.source = Some("OtherDistro".to_string());
        let container = ContainerProcessUsage {
            resource: {
                let mut row = resource(ResourceKind::Container, "web", 3.0);
                row.environment = EnvironmentKind::Docker;
                row
            },
            processes: Vec::new(),
            host_pids: vec![42],
        };

        let tree = build_tree_with_docker(
            16,
            &[windows_host("vmmemwsl", 10.0)],
            &[current, additional],
            &[],
            &[container],
        );

        assert_eq!(tree.docker_groups[0].children.len(), 1);
        assert_eq!(tree.docker_groups[0].children[0].name, "current");
        assert!(tree.docker_groups[0].children[0].source.is_none());
        assert!(tree.groups[0]
            .children
            .iter()
            .any(|child| child.name == "additional"
                && child.source.as_deref() == Some("OtherDistro")));
    }

    #[test]
    fn keeps_docker_independent_without_proven_pid_namespace_mapping() {
        let mut process = resource(ResourceKind::Process, "cc1plus", 2.0);
        process.environment = EnvironmentKind::Docker;
        let container = ContainerProcessUsage {
            resource: {
                let mut row = resource(ResourceKind::Container, "build", 3.0);
                row.environment = EnvironmentKind::Docker;
                row
            },
            processes: vec![process],
            host_pids: Vec::new(),
        };
        let tree = build_tree_with_docker(
            16,
            &[windows_host("vmmemwsl", 10.0)],
            &[],
            &[],
            &[container],
        );
        assert!(tree.groups[0].children.is_empty());
        assert_eq!(tree.docker_groups[0].children[0].name, "cc1plus");
        assert_eq!(tree.docker_groups[0].unattributed_cpu_percent, 1.0);
    }

    #[test]
    fn reports_docker_over_attribution_without_scaling_processes() {
        let mut process = resource(ResourceKind::Process, "rustc", 4.0);
        process.environment = EnvironmentKind::Docker;
        let container = ContainerProcessUsage {
            resource: {
                let mut row = resource(ResourceKind::Container, "build", 3.0);
                row.environment = EnvironmentKind::Docker;
                row
            },
            processes: vec![process],
            host_pids: Vec::new(),
        };
        let tree = build_tree_with_docker(16, &[], &[], &[], &[container]);
        assert_eq!(tree.docker_groups[0].children[0].cpu_percent, 4.0);
        assert_eq!(tree.docker_groups[0].unattributed_cpu_percent, 0.0);
        assert_eq!(tree.docker_groups[0].over_attributed_cpu_percent, 1.0);
    }
}
