//! Read-only operations on one retained observation. No collection or transport.

use crate::identity::{IdentityError, ResourceId};
use crate::model::{EnvironmentKind, HostMemory, ResourceKind, ResourceUsage};
use crate::query::Sort;
use crate::snapshot_store::{
    SnapshotError, SnapshotId, SnapshotRequest, SnapshotStore, StoredSnapshot,
};
use crate::summary::EnvironmentSummary;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryError {
    Snapshot(SnapshotError),
    Identity(IdentityError),
    InvalidHierarchy,
}

pub struct SnapshotMetadata<'a> {
    pub snapshot_id: &'a SnapshotId,
    pub captured_at: SystemTime,
    pub sample_window: Duration,
    pub warnings: &'a [String],
}

pub struct QueryReply<'a, T> {
    pub snapshot: SnapshotMetadata<'a>,
    pub data: T,
}

pub struct SystemSummary<'a> {
    pub host_logical_cpu_count: u32,
    pub host_cpu_percent: Option<f64>,
    pub host_memory: Option<HostMemory>,
    pub environments: &'a EnvironmentSummary,
}

pub struct ResourceView<'a> {
    pub resource_id: &'a ResourceId,
    pub resource: &'a ResourceUsage,
    /// Explicit observed attribution/application relationships, not PID guesses.
    pub parent_ids: Vec<&'a ResourceId>,
    pub cores_used: Option<f64>,
}

pub struct ListOptions<'a> {
    pub sort: Sort,
    pub limit: usize,
    pub environment: Option<EnvironmentKind>,
    pub resource_kind: Option<ResourceKind>,
    pub name_contains: Option<&'a str>,
}

impl Default for ListOptions<'_> {
    fn default() -> Self {
        Self {
            sort: Sort::default(),
            limit: 30,
            environment: None,
            resource_kind: None,
            name_contains: None,
        }
    }
}

struct Node<'a> {
    id: ResourceId,
    resource: &'a ResourceUsage,
    parents: Vec<usize>,
}

/// Opening selects one snapshot once. Every method returns its metadata so a
/// transport adapter can carry snapshot_id through a sequence of requests.
pub struct QueryView<'a> {
    snapshot: &'a StoredSnapshot,
    nodes: Vec<Node<'a>>,
    by_id: HashMap<String, usize>,
}

impl<'a> QueryView<'a> {
    pub fn open(
        store: &'a SnapshotStore,
        request: SnapshotRequest<'_>,
    ) -> Result<Self, QueryError> {
        let snapshot = store.get(request).map_err(QueryError::Snapshot)?;
        Self::from_snapshot(snapshot)
    }

    pub(crate) fn from_snapshot(snapshot: &'a StoredSnapshot) -> Result<Self, QueryError> {
        let source = snapshot
            .snapshot()
            .query_source
            .as_ref()
            .ok_or(QueryError::Snapshot(SnapshotError::MissingQuerySource))?;
        let mut view = Self {
            snapshot,
            nodes: Vec::new(),
            by_id: HashMap::new(),
        };
        view.add_projection(&source.resources)?;
        view.add_projection(&source.pid_resources)?;
        // Parents are source lists too: do not merge repeated groups and their
        // potentially different child sets merely because their rows agree.
        view.add_projection(source.tree.groups.iter().map(|group| &group.host))?;
        for group in &source.tree.groups {
            view.add_children(&group.host, &group.children)?;
        }
        view.add_projection(&source.tree.unmapped_children)?;
        view.add_projection(
            source
                .tree
                .docker_groups
                .iter()
                .chain(&source.tree.wslc_groups)
                .map(|group| &group.container),
        )?;
        for group in source
            .tree
            .docker_groups
            .iter()
            .chain(&source.tree.wslc_groups)
        {
            view.add_children(&group.container, &group.children)?;
        }
        view.add_projection(
            source
                .tree
                .windows_applications
                .iter()
                .map(|app| &app.resource),
        )?;
        for app in &source.tree.windows_applications {
            view.add_children(&app.resource, &app.processes)?;
        }
        // A graph is intentional: attribution can give a resource more than one
        // parent. Cycles are not an observed hierarchy and fail closed.
        let mut states = vec![0u8; view.nodes.len()];
        for index in 0..view.nodes.len() {
            view.check_acyclic(index, &mut states)?;
        }
        Ok(view)
    }

    fn check_acyclic(&self, index: usize, states: &mut [u8]) -> Result<(), QueryError> {
        if states[index] == 1 {
            return Err(QueryError::InvalidHierarchy);
        }
        if states[index] == 2 {
            return Ok(());
        }
        states[index] = 1;
        for &parent in &self.nodes[index].parents {
            self.check_acyclic(parent, states)?;
        }
        states[index] = 2;
        Ok(())
    }

    fn add_projection(
        &mut self,
        rows: impl IntoIterator<Item = &'a ResourceUsage>,
    ) -> Result<(), QueryError> {
        let mut seen = HashSet::new();
        for row in rows {
            let index = self.add(row)?;
            if !seen.insert(index) {
                return Err(QueryError::Identity(IdentityError::AmbiguousResource));
            }
        }
        Ok(())
    }

    fn add(&mut self, resource: &'a ResourceUsage) -> Result<usize, QueryError> {
        let id = self.snapshot.resource_identity(resource).resource_id();
        if let Some(&index) = self.by_id.get(id.as_str()) {
            if self.nodes[index].resource != resource {
                return Err(QueryError::Identity(IdentityError::AmbiguousResource));
            }
            return Ok(index);
        }
        let index = self.nodes.len();
        self.by_id.insert(id.as_str().to_owned(), index);
        self.nodes.push(Node {
            id,
            resource,
            parents: Vec::new(),
        });
        Ok(index)
    }

    fn add_children(
        &mut self,
        parent: &'a ResourceUsage,
        children: &'a [ResourceUsage],
    ) -> Result<(), QueryError> {
        let parent = self.add(parent)?;
        self.add_projection(children)?;
        for child in children {
            let child = self.add(child)?;
            if child == parent {
                return Err(QueryError::InvalidHierarchy);
            }
            if !self.nodes[child].parents.contains(&parent) {
                self.nodes[child].parents.push(parent);
            }
        }
        Ok(())
    }

    fn metadata(&self) -> SnapshotMetadata<'_> {
        SnapshotMetadata {
            snapshot_id: self.snapshot.id(),
            captured_at: self.snapshot.captured_at(),
            sample_window: self.snapshot.sample_window(),
            warnings: &self.snapshot.snapshot().warnings,
        }
    }

    fn resource_view(&self, index: usize) -> ResourceView<'_> {
        let node = &self.nodes[index];
        let cpu_count = self.snapshot.snapshot().host_logical_cpu_count;
        let cores = node.resource.cpu_percent * f64::from(cpu_count) / 100.0;
        ResourceView {
            resource_id: &node.id,
            resource: node.resource,
            parent_ids: node
                .parents
                .iter()
                .map(|&parent| &self.nodes[parent].id)
                .collect(),
            cores_used: (cpu_count > 0 && cores.is_finite() && cores >= 0.0).then_some(cores),
        }
    }

    fn index(&self, id: &str) -> Result<usize, QueryError> {
        self.by_id
            .get(id)
            .copied()
            .ok_or(QueryError::Identity(IdentityError::UnknownResource))
    }

    pub fn get_system_summary(&self) -> QueryReply<'_, SystemSummary<'_>> {
        let snapshot = self.snapshot.snapshot();
        QueryReply {
            snapshot: self.metadata(),
            data: SystemSummary {
                host_logical_cpu_count: snapshot.host_logical_cpu_count,
                host_cpu_percent: snapshot.host_cpu_percent,
                host_memory: snapshot.host_memory,
                environments: &snapshot.environment_summary,
            },
        }
    }

    pub fn inspect_resource(
        &self,
        resource_id: &str,
    ) -> Result<QueryReply<'_, ResourceView<'_>>, QueryError> {
        Ok(QueryReply {
            snapshot: self.metadata(),
            data: self.resource_view(self.index(resource_id)?),
        })
    }

    pub fn list_resources(
        &self,
        options: &ListOptions<'_>,
    ) -> QueryReply<'_, Vec<ResourceView<'_>>> {
        self.list(None, options)
    }

    pub fn list_children(
        &self,
        parent: &str,
        options: &ListOptions<'_>,
    ) -> Result<QueryReply<'_, Vec<ResourceView<'_>>>, QueryError> {
        Ok(self.list(Some(self.index(parent)?), options))
    }

    fn list(
        &self,
        parent: Option<usize>,
        options: &ListOptions<'_>,
    ) -> QueryReply<'_, Vec<ResourceView<'_>>> {
        let mut indices: Vec<_> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                parent.is_none_or(|parent| node.parents.contains(&parent))
                    && options
                        .environment
                        .is_none_or(|env| node.resource.environment == env)
                    && options
                        .resource_kind
                        .is_none_or(|kind| node.resource.kind == kind)
                    && options
                        .name_contains
                        .is_none_or(|name| node.resource.name.contains(name))
            })
            .map(|(index, _)| index)
            .collect();
        // Reuse the ordering shared by CLI/TUI, including stable tie-breaks.
        indices.sort_by(|&a, &b| {
            options
                .sort
                .compare(self.nodes[a].resource, self.nodes[b].resource)
        });
        indices.truncate(options.limit);
        QueryReply {
            snapshot: self.metadata(),
            data: indices.into_iter().map(|i| self.resource_view(i)).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::{
        AttributionGroup, AttributionTree, DockerAttributionGroup, MappingStatus,
    };
    use crate::identity::IdentityScope;
    use crate::model::WindowsApplicationUsage;
    use crate::monitor::MonitorSnapshot;
    use crate::query::{QuerySource, SortKey, SortOrder};
    use crate::snapshot_store::SampleTiming;
    use std::num::NonZeroUsize;
    use std::time::Instant;

    fn row(
        env: EnvironmentKind,
        kind: ResourceKind,
        id: &str,
        cpu: f64,
        memory: u64,
    ) -> ResourceUsage {
        ResourceUsage {
            environment: env,
            kind,
            source: None,
            id: id.into(),
            pid: (kind == ResourceKind::Process).then(|| id.parse().unwrap()),
            start_id: (kind == ResourceKind::Process).then_some(100),
            ppid: None,
            name: id.into(),
            args: Some(format!("command {id}")),
            cpu_percent: cpu,
            cpu_time_seconds: Some(2.0),
            memory_bytes: memory,
        }
    }

    fn fixture() -> MonitorSnapshot {
        use EnvironmentKind::*;
        use ResourceKind::*;
        let host = row(Windows, Host, "42", 30.0, 1000);
        let app = row(Windows, Application, "editor", 3.0, 500);
        let win = row(Windows, Process, "43", 3.0, 500);
        let wsl = row(Wsl, Process, "4", 1.0, 600);
        let docker = row(Docker, Container, "docker-full-id", 10.0, 300);
        let mut child = row(Docker, Process, "7", 9.0, 200);
        child.source = Some(docker.id.clone());
        child.start_id = None;
        let wslc = row(WslContainer, Container, "wslc-full-id", 5.0, 400);
        let mut other = row(WslContainer, Process, "7", 4.0, 350);
        other.source = Some(wslc.id.clone());
        other.start_id = None;
        let tree = AttributionTree {
            host_logical_cpu_count: 16,
            groups: vec![AttributionGroup {
                name: "WSL VM".into(),
                host,
                cpu_percent: 30.0,
                children: vec![wsl.clone(), docker.clone()],
                known_children_cpu_percent: 11.0,
                unattributed_cpu_percent: 19.0,
                over_attributed_cpu_percent: 0.0,
                mapping_status: MappingStatus::Resolved,
            }],
            unmapped_children: vec![],
            docker_groups: vec![DockerAttributionGroup {
                container: docker.clone(),
                children: vec![child.clone()],
                unattributed_cpu_percent: 1.0,
                over_attributed_cpu_percent: 0.0,
            }],
            wslc_groups: vec![DockerAttributionGroup {
                container: wslc.clone(),
                children: vec![other.clone()],
                unattributed_cpu_percent: 1.0,
                over_attributed_cpu_percent: 0.0,
            }],
            windows_applications: vec![WindowsApplicationUsage {
                resource: app.clone(),
                processes: vec![win.clone()],
            }],
        };
        MonitorSnapshot {
            host_cpu_percent: Some(40.0),
            host_memory: Some(HostMemory {
                total_bytes: 2000,
                available_bytes: 1000,
            }),
            host_history: Default::default(),
            environment_summary: Default::default(),
            sort: Sort::default(),
            host_logical_cpu_count: 16,
            resources: vec![],
            pid_resources: vec![],
            tree: tree.clone(),
            warnings: vec!["partial observation".into()],
            query_source: Some(QuerySource {
                resources: vec![
                    app,
                    wsl.clone(),
                    docker.clone(),
                    wslc.clone(),
                    child.clone(),
                    other.clone(),
                ],
                pid_resources: vec![win, wsl, docker, wslc, child, other],
                tree,
            }),
        }
    }

    fn cache() -> SnapshotStore {
        SnapshotStore::new(
            IdentityScope::new("query-test-session".into()).unwrap(),
            NonZeroUsize::new(4).unwrap(),
            Duration::from_secs(60),
        )
        .unwrap()
    }

    fn insert(store: &mut SnapshotStore, snapshot: MonitorSnapshot) -> SnapshotId {
        let now = Instant::now();
        store
            .insert(
                snapshot,
                SampleTiming::new(now - Duration::from_secs(3), now, SystemTime::now()).unwrap(),
            )
            .unwrap()
    }

    fn id<'a>(view: &'a QueryView<'_>, native: &str) -> &'a str {
        view.nodes
            .iter()
            .find(|node| node.resource.id == native)
            .unwrap()
            .id
            .as_str()
    }

    #[test]
    fn four_operations_share_metadata_and_untruncated_numeric_observations() {
        let mut store = cache();
        let snapshot_id = insert(&mut store, fixture());
        let view = QueryView::open(&store, SnapshotRequest::Id(snapshot_id.as_str())).unwrap();
        let list = view.list_resources(&ListOptions::default());
        assert_eq!(list.data.len(), 8); // Cross-view duplicates collapse; tree-only host and PID remain.
        assert_eq!(list.snapshot.snapshot_id, &snapshot_id);
        assert_eq!(list.snapshot.sample_window, Duration::from_secs(3));
        assert_eq!(list.snapshot.warnings, ["partial observation"]);
        let summary = view.get_system_summary();
        assert_eq!(summary.snapshot.snapshot_id, list.snapshot.snapshot_id);
        assert_eq!(summary.snapshot.captured_at, list.snapshot.captured_at);
        assert_eq!(summary.data.host_cpu_percent, Some(40.0));
        assert_eq!(summary.data.host_logical_cpu_count, 16);
        assert_eq!(summary.data.host_memory.unwrap().used_bytes(), Some(1000));
        assert!(summary.data.environments.0.iter().all(Option::is_none));
        let details = view.inspect_resource(id(&view, "43")).unwrap();
        assert_eq!(details.snapshot.snapshot_id, &snapshot_id);
        assert_eq!(details.data.resource.args.as_deref(), Some("command 43"));
        assert_eq!(details.data.resource.memory_bytes, 500);
        assert_eq!(details.data.cores_used, Some(0.48));
        let children = view
            .list_children(id(&view, "editor"), &ListOptions::default())
            .unwrap();
        assert_eq!(children.snapshot.snapshot_id, &snapshot_id);
        assert_eq!(children.data[0].resource_id, details.data.resource_id);
        assert_eq!(details.data.parent_ids[0].as_str(), id(&view, "editor"));
    }

    #[test]
    fn filtering_precedes_limit_and_sort_uses_the_shared_comparator() {
        let mut store = cache();
        let sid = insert(&mut store, fixture());
        let view = QueryView::open(&store, SnapshotRequest::Id(sid.as_str())).unwrap();
        for key in [SortKey::Cpu, SortKey::Memory, SortKey::Name] {
            for order in [SortOrder::Asc, SortOrder::Desc] {
                let options = ListOptions {
                    sort: Sort { key, order },
                    ..Default::default()
                };
                let rows = view.list_resources(&options);
                assert!(rows.data.windows(2).all(|pair| options
                    .sort
                    .compare(pair[0].resource, pair[1].resource)
                    .is_le()));
            }
        }
        let options = ListOptions {
            environment: Some(EnvironmentKind::Docker),
            resource_kind: Some(ResourceKind::Process),
            name_contains: Some("7"),
            limit: 1,
            ..Default::default()
        };
        let result = view.list_resources(&options);
        assert_eq!(result.data.len(), 1);
        assert_eq!(
            result.data[0].resource.source.as_deref(),
            Some("docker-full-id")
        );
        assert_eq!(
            view.list_resources(&ListOptions {
                limit: 0,
                ..Default::default()
            })
            .data
            .len(),
            0
        );
        assert!(view
            .list_resources(&ListOptions {
                name_contains: Some("EDITOR"),
                ..Default::default()
            })
            .data
            .is_empty());
        assert_eq!(view.get_system_summary().data.host_cpu_percent, Some(40.0));
    }

    #[test]
    fn attribution_children_keep_namespaces_and_do_not_invent_residual_rows() {
        let mut store = cache();
        let sid = insert(&mut store, fixture());
        let view = QueryView::open(&store, SnapshotRequest::Id(sid.as_str())).unwrap();
        let docker = view
            .list_children(id(&view, "docker-full-id"), &ListOptions::default())
            .unwrap();
        let wslc = view
            .list_children(id(&view, "wslc-full-id"), &ListOptions::default())
            .unwrap();
        assert_eq!(docker.data.len(), 1);
        assert_eq!(wslc.data.len(), 1);
        assert_ne!(docker.data[0].resource_id, wslc.data[0].resource_id);
        assert_eq!(docker.data[0].resource.environment, EnvironmentKind::Docker);
        assert_eq!(
            wslc.data[0].resource.environment,
            EnvironmentKind::WslContainer
        );
        assert_eq!(
            view.list_children(id(&view, "42"), &ListOptions::default())
                .unwrap()
                .data
                .len(),
            2
        );
        assert!(view
            .list_children(docker.data[0].resource_id.as_str(), &ListOptions::default())
            .unwrap()
            .data
            .is_empty());
        assert_eq!(
            view.inspect_resource("43").err(),
            Some(QueryError::Identity(IdentityError::UnknownResource))
        );
        assert_eq!(
            view.list_children("missing", &ListOptions::default()).err(),
            Some(QueryError::Identity(IdentityError::UnknownResource))
        );
    }

    #[test]
    fn pinned_operations_stay_on_the_selected_snapshot() {
        let mut store = cache();
        let old = insert(&mut store, fixture());
        let mut later = fixture();
        later.host_cpu_percent = Some(80.0);
        let newest = insert(&mut store, later);
        let pinned = QueryView::open(&store, SnapshotRequest::Id(old.as_str())).unwrap();
        assert_eq!(
            pinned.get_system_summary().data.host_cpu_percent,
            Some(40.0)
        );
        let latest = QueryView::open(
            &store,
            SnapshotRequest::Latest {
                max_age: Duration::from_secs(60),
            },
        )
        .unwrap();
        assert_eq!(latest.get_system_summary().snapshot.snapshot_id, &newest);
        assert_eq!(
            latest.get_system_summary().data.host_cpu_percent,
            Some(80.0)
        );
        let old_child = pinned
            .list_children(id(&pinned, "docker-full-id"), &ListOptions::default())
            .unwrap();
        let old_child_id = old_child.data[0].resource_id.as_str();
        assert!(pinned.inspect_resource(old_child_id).is_ok());
        assert_eq!(
            latest.inspect_resource(old_child_id).err(),
            Some(QueryError::Identity(IdentityError::UnknownResource))
        );
        assert!(latest.inspect_resource(id(&pinned, "43")).is_ok());
        assert_eq!(
            QueryView::open(&store, SnapshotRequest::Id("missing")).err(),
            Some(QueryError::Snapshot(SnapshotError::SnapshotUnavailable))
        );
        store.rotate_namespace().unwrap();
        assert_eq!(
            QueryView::open(&store, SnapshotRequest::Id(old.as_str())).err(),
            Some(QueryError::Snapshot(SnapshotError::SnapshotUnavailable))
        );
    }

    #[test]
    fn conflicting_copies_duplicate_rows_and_cycles_fail_closed() {
        let mut conflicting = fixture();
        conflicting.query_source.as_mut().unwrap().pid_resources[1].memory_bytes += 1;
        let mut duplicate = fixture();
        let source = duplicate.query_source.as_mut().unwrap();
        source.resources.push(source.resources[0].clone());
        for sample in [conflicting, duplicate] {
            let mut store = cache();
            let sid = insert(&mut store, sample);
            assert_eq!(
                QueryView::open(&store, SnapshotRequest::Id(sid.as_str())).err(),
                Some(QueryError::Identity(IdentityError::AmbiguousResource))
            );
        }
        let mut sample = fixture();
        let tree = &mut sample.query_source.as_mut().unwrap().tree;
        tree.docker_groups[0]
            .children
            .push(tree.groups[0].host.clone()); // VM -> Docker -> VM
        let mut store = cache();
        let sid = insert(&mut store, sample);
        assert_eq!(
            QueryView::open(&store, SnapshotRequest::Id(sid.as_str())).err(),
            Some(QueryError::InvalidHierarchy)
        );
    }

    #[test]
    fn shared_child_has_multiple_parents_without_duplicate_listing() {
        let mut sample = fixture();
        let tree = &mut sample.query_source.as_mut().unwrap().tree;
        let mut second = tree.groups[0].clone();
        second.host.id = "44".into();
        second.host.name = "other host".into();
        second.children = vec![tree.docker_groups[0].container.clone()];
        tree.groups.push(second);
        let mut store = cache();
        let sid = insert(&mut store, sample);
        let view = QueryView::open(&store, SnapshotRequest::Id(sid.as_str())).unwrap();
        let container = view.inspect_resource(id(&view, "docker-full-id")).unwrap();
        assert_eq!(container.data.parent_ids.len(), 2);
        let children = view
            .list_children(id(&view, "44"), &ListOptions::default())
            .unwrap();
        assert_eq!(children.data.len(), 1);
        assert_eq!(children.data[0].resource_id, container.data.resource_id);
        let options = ListOptions {
            resource_kind: Some(ResourceKind::Container),
            sort: Sort {
                key: SortKey::Memory,
                order: SortOrder::Asc,
            },
            limit: 1,
            ..Default::default()
        };
        assert_eq!(
            view.list_children(id(&view, "42"), &options).unwrap().data[0].resource_id,
            container.data.resource_id
        );
        assert_eq!(
            view.list_resources(&ListOptions {
                name_contains: Some("docker-full-id"),
                ..Default::default()
            })
            .data
            .len(),
            1
        );
    }

    #[test]
    fn duplicate_parents_in_each_hierarchy_list_are_ambiguous() {
        for list in 0..4 {
            for different_children in [false, true] {
                let mut sample = fixture();
                let tree = &mut sample.query_source.as_mut().unwrap().tree;
                match list {
                    0 => {
                        let mut duplicate = tree.groups[0].clone();
                        if different_children {
                            duplicate.children.clear();
                        }
                        tree.groups.push(duplicate);
                    }
                    1 | 2 => {
                        let groups = if list == 1 {
                            &mut tree.docker_groups
                        } else {
                            &mut tree.wslc_groups
                        };
                        let mut duplicate = groups[0].clone();
                        if different_children {
                            duplicate.children.clear();
                        }
                        groups.push(duplicate);
                    }
                    _ => {
                        let mut duplicate = tree.windows_applications[0].clone();
                        if different_children {
                            duplicate.processes.clear();
                        }
                        tree.windows_applications.push(duplicate);
                    }
                }
                let mut store = cache();
                let sid = insert(&mut store, sample);
                assert_eq!(
                    QueryView::open(&store, SnapshotRequest::Id(sid.as_str())).err(),
                    Some(QueryError::Identity(IdentityError::AmbiguousResource)),
                    "list {list}, different children: {different_children}"
                );
            }
        }
    }
}
