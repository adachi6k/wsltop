//! Presentation-independent ordering and projections of a collected snapshot.
use crate::attribution::{self, AttributionTree};
use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};
use std::cmp::Ordering;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortKey {
    #[default]
    Cpu,
    Memory,
    Name,
}

impl SortKey {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "cpu" => Ok(Self::Cpu),
            "memory" => Ok(Self::Memory),
            "name" => Ok(Self::Name),
            _ => Err(format!(
                "invalid sort key {value:?}; expected cpu, memory or name"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::Memory => "memory",
            Self::Name => "name",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SortOrder {
    Asc,
    #[default]
    Desc,
}

impl SortOrder {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "asc" => Ok(Self::Asc),
            "desc" => Ok(Self::Desc),
            _ => Err(format!(
                "invalid sort order {value:?}; expected asc or desc"
            )),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }

    pub fn reverse(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sort {
    pub key: SortKey,
    pub order: SortOrder,
}

impl Sort {
    pub fn compare(self, a: &ResourceUsage, b: &ResourceUsage) -> Ordering {
        // Keep the existing CPU/memory ranking. Missing/non-finite CPU values
        // are treated as zero; display scaling never participates in ordering.
        let cpu = |row: &ResourceUsage| {
            if row.cpu_percent.is_finite() {
                row.cpu_percent
            } else {
                0.0
            }
        };
        let selected = match self.key {
            SortKey::Cpu => cpu(a)
                .total_cmp(&cpu(b))
                .then(a.memory_bytes.cmp(&b.memory_bytes)),
            SortKey::Memory => a
                .memory_bytes
                .cmp(&b.memory_bytes)
                .then(cpu(a).total_cmp(&cpu(b))),
            SortKey::Name => a.name.cmp(&b.name),
        };
        let selected = match self.order {
            SortOrder::Asc => selected,
            SortOrder::Desc => selected.reverse(),
        };
        // Identity tie-breaks stay ascending in either direction. Names use
        // case-sensitive Rust string ordering, consistently across adapters.
        selected
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| (a.environment as u8).cmp(&(b.environment as u8)))
            .then_with(|| a.kind.as_str().cmp(b.kind.as_str()))
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.id.cmp(&b.id))
            .then_with(|| a.start_id.cmp(&b.start_id))
    }

    pub fn resources(self, rows: &mut [ResourceUsage]) {
        rows.sort_by(|a, b| self.compare(a, b));
    }
}

#[derive(Debug, Clone)]
pub struct ResourceQuery {
    pub sort: Sort,
    pub limit: usize,
    pub container_process_limit: usize,
    pub show_container_processes: bool,
    pub show_wsl_host: bool,
    pub hide_infra: bool,
}

impl Default for ResourceQuery {
    fn default() -> Self {
        Self {
            sort: Sort::default(),
            limit: 30,
            container_process_limit: 5,
            show_container_processes: false,
            show_wsl_host: false,
            hide_infra: false,
        }
    }
}

fn container_process(row: &ResourceUsage) -> bool {
    matches!(
        row.environment,
        EnvironmentKind::Docker | EnvironmentKind::WslContainer
    ) && row.kind == ResourceKind::Process
        && row.source.is_some()
}

impl ResourceQuery {
    pub fn flat(&self, rows: &[ResourceUsage]) -> Vec<ResourceUsage> {
        let (mut children, mut parents): (Vec<_>, Vec<_>) = rows
            .iter()
            .filter(|row| self.show_wsl_host || !attribution::is_host_resource(row))
            .filter(|row| !self.hide_infra || row.kind != ResourceKind::Infra)
            .cloned()
            .partition(container_process);
        self.sort.resources(&mut parents);
        self.sort.resources(&mut children);
        parents.truncate(self.limit);
        let mut result = Vec::new();
        for parent in parents {
            let matched = if self.show_container_processes && parent.kind == ResourceKind::Container
            {
                children
                    .iter()
                    .filter(|child| {
                        child.environment == parent.environment
                            && child.source.as_deref() == Some(parent.id.as_str())
                    })
                    .take(self.container_process_limit)
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            result.push(parent);
            result.extend(matched);
        }
        result
    }

    pub fn tree(&self, source: &AttributionTree) -> AttributionTree {
        let mut tree = source.clone();
        if self.hide_infra {
            attribution::hide_infra(&mut tree);
        }
        tree.groups
            .sort_by(|a, b| self.sort.compare(&a.host, &b.host));
        for group in &mut tree.groups {
            self.sort.resources(&mut group.children);
        }
        self.sort.resources(&mut tree.unmapped_children);
        for groups in [&mut tree.docker_groups, &mut tree.wslc_groups] {
            groups.sort_by(|a, b| self.sort.compare(&a.container, &b.container));
            for group in groups {
                self.sort.resources(&mut group.children);
            }
        }
        tree.windows_applications
            .sort_by(|a, b| self.sort.compare(&a.resource, &b.resource));
        for app in &mut tree.windows_applications {
            self.sort.resources(&mut app.processes);
        }
        // Residuals and accounting values are not recomputed or ranked as peers.
        tree
    }
}

/// Retain the untruncated observation so a new query cannot lose candidates
/// discarded by an earlier sort/limit. This is local view state, not an Agent API.
pub struct QuerySource {
    pub resources: Vec<ResourceUsage>,
    pub pid_resources: Vec<ResourceUsage>,
    pub tree: AttributionTree,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::attribution::{AttributionGroup, DockerAttributionGroup, MappingStatus};
    use crate::model::WindowsApplicationUsage;

    pub(crate) fn row(name: &str, cpu: f64, memory: u64) -> ResourceUsage {
        ResourceUsage {
            environment: EnvironmentKind::Wsl,
            kind: ResourceKind::Process,
            source: None,
            id: name.into(),
            pid: Some(1),
            start_id: Some(1),
            ppid: None,
            name: name.into(),
            args: None,
            cpu_percent: cpu,
            cpu_time_seconds: Some(2.0),
            memory_bytes: memory,
        }
    }

    fn names(rows: &[ResourceUsage]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn keys_and_directions_use_numeric_values_and_case_sensitive_names() {
        let rows = vec![
            row("bravo", 10.0, 100),
            row("charlie", 1.0, 900),
            row("Alpha", 5.0, 300),
        ];
        for (key, asc) in [
            (SortKey::Cpu, vec!["charlie", "Alpha", "bravo"]),
            (SortKey::Memory, vec!["bravo", "Alpha", "charlie"]),
            (SortKey::Name, vec!["Alpha", "bravo", "charlie"]),
        ] {
            for order in [SortOrder::Asc, SortOrder::Desc] {
                let query = ResourceQuery {
                    sort: Sort { key, order },
                    ..Default::default()
                };
                let mut expected = asc.clone();
                if order == SortOrder::Desc {
                    expected.reverse();
                }
                assert_eq!(names(&query.flat(&rows)), expected);
            }
        }
        assert_eq!(
            names(&ResourceQuery::default().flat(&rows)),
            ["bravo", "Alpha", "charlie"]
        );
        assert_eq!(rows[0].cpu_time_seconds, Some(2.0));
    }

    #[test]
    fn ties_are_deterministic_and_default_cpu_ties_prefer_memory() {
        let mut first = row("same", 2.0, 20);
        first.id = "1".into();
        let mut second = first.clone();
        second.id = "2".into();
        for order in [SortOrder::Asc, SortOrder::Desc] {
            let mut rows = vec![second.clone(), first.clone()];
            Sort {
                key: SortKey::Name,
                order,
            }
            .resources(&mut rows);
            assert_eq!(rows[0].id, "1");
        }
        let query = ResourceQuery::default();
        assert_eq!(
            names(&query.flat(&[row("small", 2.0, 1), row("large", 2.0, 100)])),
            ["large", "small"]
        );
        assert_eq!(
            names(&query.flat(&[row("nan", f64::NAN, 1), row("busy", 1.0, 1)])),
            ["busy", "nan"]
        );
    }

    #[test]
    fn limits_follow_sorting_and_children_match_environment_as_well_as_id() {
        let mut docker = row("docker", 10.0, 10);
        docker.environment = EnvironmentKind::Docker;
        docker.kind = ResourceKind::Container;
        docker.id = "shared".into();
        let mut wslc = docker.clone();
        wslc.environment = EnvironmentKind::WslContainer;
        wslc.name = "wslc".into();
        wslc.memory_bytes = 100;
        let mut docker_child = row("wrong-runtime", 100.0, 1000);
        docker_child.environment = EnvironmentKind::Docker;
        docker_child.source = Some("shared".into());
        let mut cpu_child = docker_child.clone();
        cpu_child.environment = EnvironmentKind::WslContainer;
        cpu_child.name = "busy".into();
        cpu_child.memory_bytes = 1;
        let mut memory_child = cpu_child.clone();
        memory_child.name = "big".into();
        memory_child.cpu_percent = 1.0;
        memory_child.memory_bytes = 200;
        let rows = vec![docker_child, cpu_child, memory_child, docker, wslc];
        let mut query = ResourceQuery {
            sort: Sort {
                key: SortKey::Memory,
                order: SortOrder::Desc,
            },
            limit: 1,
            container_process_limit: 1,
            show_container_processes: true,
            ..Default::default()
        };
        assert_eq!(names(&query.flat(&rows)), ["wslc", "big"]);
        query.limit = 2;
        assert_eq!(
            names(&query.flat(&rows)),
            ["wslc", "big", "docker", "wrong-runtime"]
        );
        query.show_container_processes = false;
        assert_eq!(names(&query.flat(&rows)), ["wslc", "docker"]);
        query.limit = 0;
        assert!(query.flat(&rows).is_empty());
    }

    #[test]
    fn tree_sorts_each_level_without_changing_accounting_or_source() {
        let small = row("small", 8.0, 10);
        let large = row("large", 2.0, 100);
        let group = |host: ResourceUsage| AttributionGroup {
            name: host.name.clone(),
            cpu_percent: host.cpu_percent,
            host,
            children: vec![small.clone(), large.clone()],
            known_children_cpu_percent: 10.0,
            unattributed_cpu_percent: 3.0,
            over_attributed_cpu_percent: 4.0,
            mapping_status: MappingStatus::Resolved,
        };
        let container = |resource: ResourceUsage| DockerAttributionGroup {
            container: resource,
            children: vec![small.clone(), large.clone()],
            unattributed_cpu_percent: 3.0,
            over_attributed_cpu_percent: 4.0,
        };
        let tree = AttributionTree {
            host_logical_cpu_count: 16,
            groups: vec![group(small.clone()), group(large.clone())],
            unmapped_children: vec![small.clone(), large.clone()],
            docker_groups: vec![container(small.clone()), container(large.clone())],
            wslc_groups: vec![container(small.clone()), container(large.clone())],
            windows_applications: vec![
                WindowsApplicationUsage {
                    resource: small.clone(),
                    processes: vec![small.clone(), large.clone()],
                },
                WindowsApplicationUsage {
                    resource: large.clone(),
                    processes: vec![small.clone(), large.clone()],
                },
            ],
        };
        let query = ResourceQuery {
            sort: Sort {
                key: SortKey::Memory,
                order: SortOrder::Desc,
            },
            limit: 1,
            container_process_limit: 1,
            ..Default::default()
        };
        let view = query.tree(&tree);
        assert_eq!(view.groups[0].host.name, "large");
        assert_eq!(names(&view.groups[0].children), ["large", "small"]);
        assert_eq!(names(&view.unmapped_children), ["large", "small"]);
        for groups in [&view.docker_groups, &view.wslc_groups] {
            assert_eq!(groups[0].container.name, "large");
            assert_eq!(names(&groups[0].children), ["large", "small"]);
            assert_eq!(groups[0].unattributed_cpu_percent, 3.0);
            assert_eq!(groups[0].over_attributed_cpu_percent, 4.0);
        }
        assert_eq!(view.windows_applications[0].resource.name, "large");
        assert_eq!(
            names(&view.windows_applications[0].processes),
            ["large", "small"]
        );
        assert_eq!(view.groups[0].known_children_cpu_percent, 10.0);
        assert_eq!(tree.groups[0].host.name, "small");
        assert_eq!(view.groups[0].children[0].memory_bytes, 100);
        assert_eq!(view.groups[0].children[0].cpu_percent, 2.0);
    }
}
