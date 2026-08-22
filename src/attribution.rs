use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};
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
}

pub fn build_tree(
    host_logical_cpu_count: u32,
    hosts: &[ResourceUsage],
    wsl_children: &[ResourceUsage],
    wslc_children: &[ResourceUsage],
) -> AttributionTree {
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
        wsl_children,
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
    }
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
}

#[cfg(test)]
mod tests {
    use super::{build_tree, make_group, MappingStatus};
    use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};

    fn resource(kind: ResourceKind, name: &str, cpu_percent: f64) -> ResourceUsage {
        ResourceUsage {
            environment: EnvironmentKind::Wsl,
            kind,
            id: name.to_string(),
            pid: Some(1),
            name: name.to_string(),
            cpu_percent,
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
        let tree = build_tree(
            16,
            &[windows_host("vmmemwslc-cli-user", 10.0)],
            &[],
            &[container],
        );

        assert_eq!(tree.groups.len(), 1);
        assert_eq!(tree.groups[0].mapping_status, MappingStatus::Resolved);
        assert_eq!(tree.groups[0].children[0].kind, ResourceKind::Container);
        assert!(tree.unmapped_children.is_empty());
    }

    #[test]
    fn does_not_guess_between_multiple_wslc_hosts() {
        let container = resource(ResourceKind::Container, "demo", 4.0);
        let tree = build_tree(
            16,
            &[
                windows_host("vmmemwslc-cli-a", 10.0),
                windows_host("vmmemwslc-cli-b", 10.0),
            ],
            &[],
            &[container],
        );

        assert_eq!(tree.groups.len(), 2);
        assert!(tree
            .groups
            .iter()
            .all(|group| group.mapping_status == MappingStatus::Unresolved
                && group.children.is_empty()));
        assert_eq!(tree.unmapped_children.len(), 1);
    }
}
