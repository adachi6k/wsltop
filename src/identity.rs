//! Scoped resource identities and read-only lookup for the future Query API.
//!
//! The caller owns namespace and observation epochs; see docs/resource-identity.md.
//! Matching observations is not permission to act and does not collect live state.

use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};
use serde::Serialize;

/// A caller-supplied namespace incarnation, including service/host and collector
/// configuration. Rotate it on a host/distro/container namespace restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IdentityScope(String);

impl IdentityScope {
    pub fn new(epoch: String) -> Result<Self, IdentityError> {
        if epoch.is_empty() {
            return Err(IdentityError::EmptyEpoch);
        }
        Ok(Self(epoch))
    }
}

/// Unique within a scope; identifies one immutable, unfiltered observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObservationId(String);

impl ObservationId {
    pub fn new(epoch: String) -> Result<Self, IdentityError> {
        if epoch.is_empty() {
            return Err(IdentityError::EmptyEpoch);
        }
        Ok(Self(epoch))
    }
}

/// Clients must compare/pass this string intact, never parse it. Not a secret,
/// capability, or authentication token. No deserializer accepts a claimed identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ResourceId(String);

impl ResourceId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
enum Incarnation {
    Process { pid: u32, start_id: u64 },
    Observation(ObservationId),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceIdentity {
    scope: IdentityScope,
    environment: EnvironmentKind,
    kind: ResourceKind,
    source: Option<String>,
    native_id: String,
    incarnation: Incarnation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityError {
    EmptyEpoch,
    UnknownResource,
    AmbiguousResource,
    UnsupportedResource,
    UnverifiableProcess,
    StaleResource,
}

impl ResourceIdentity {
    pub fn observed(
        scope: &IdentityScope,
        observation: &ObservationId,
        row: &ResourceUsage,
    ) -> Self {
        let incarnation = match (row.kind, row.pid, row.start_id) {
            (ResourceKind::Process | ResourceKind::Infra, Some(pid), Some(start_id)) => {
                Incarnation::Process { pid, start_id }
            }
            _ => Incarnation::Observation(observation.clone()),
        };
        Self {
            scope: scope.clone(),
            environment: row.environment,
            kind: row.kind,
            source: row.source.clone(),
            native_id: row.id.clone(),
            incarnation,
        }
    }

    pub fn resource_id(&self) -> ResourceId {
        // Collision-free encoding of a structured key, rather than a hash or
        // delimiter concatenation. This private versioned representation can
        // change before the external Query API is introduced.
        let bytes = serde_json::to_vec(self).expect("identity contains only serializable values");
        let mut encoded = String::with_capacity(3 + bytes.len() * 2);
        encoded.push_str("r1-");
        for byte in bytes {
            use std::fmt::Write;
            write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
        }
        ResourceId(encoded)
    }

    /// Compare with a separately collected process observation. A future action
    /// backend must collect fresh state and close the check/act race itself.
    pub fn matches_process_observation(
        &self,
        scope: &IdentityScope,
        row: &ResourceUsage,
    ) -> Result<(), IdentityError> {
        if !matches!(self.kind, ResourceKind::Process | ResourceKind::Infra) {
            return Err(IdentityError::UnsupportedResource);
        }
        let Incarnation::Process { pid, start_id } = self.incarnation else {
            return Err(IdentityError::UnverifiableProcess);
        };
        if &self.scope != scope
            || self.environment != row.environment
            || self.kind != row.kind
            || self.source != row.source
            || self.native_id != row.id
            || row.pid != Some(pid)
        {
            return Err(IdentityError::StaleResource);
        }
        match row.start_id {
            Some(current) if current == start_id => Ok(()),
            Some(_) => Err(IdentityError::StaleResource),
            None => Err(IdentityError::UnverifiableProcess),
        }
    }
}

pub struct IndexedResource<'a> {
    pub resource_id: ResourceId,
    pub identity: ResourceIdentity,
    pub resource: &'a ResourceUsage,
}

/// Borrows an immutable collection. Duplicate identities fail closed at lookup;
/// callers must not choose an arbitrary row when collector observations conflict.
pub struct ResourceIndex<'a> {
    entries: Vec<IndexedResource<'a>>,
}

impl<'a> ResourceIndex<'a> {
    pub fn new(
        scope: &IdentityScope,
        observation: &ObservationId,
        rows: &'a [ResourceUsage],
    ) -> Self {
        let entries = rows
            .iter()
            .map(|resource| {
                let identity = ResourceIdentity::observed(scope, observation, resource);
                IndexedResource {
                    resource_id: identity.resource_id(),
                    identity,
                    resource,
                }
            })
            .collect();
        Self { entries }
    }

    pub fn entries(&self) -> &[IndexedResource<'a>] {
        &self.entries
    }

    pub fn lookup(&self, resource_id: &str) -> Result<&IndexedResource<'a>, IdentityError> {
        let mut matches = self
            .entries
            .iter()
            .filter(|entry| entry.resource_id.as_str() == resource_id);
        let found = matches.next().ok_or(IdentityError::UnknownResource)?;
        if matches.next().is_some() {
            return Err(IdentityError::AmbiguousResource);
        }
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(value: &str) -> IdentityScope {
        IdentityScope::new(value.into()).unwrap()
    }

    fn observation(value: &str) -> ObservationId {
        ObservationId::new(value.into()).unwrap()
    }

    fn process() -> ResourceUsage {
        ResourceUsage {
            environment: EnvironmentKind::Wsl,
            source: Some("Ubuntu".into()),
            kind: ResourceKind::Process,
            id: "42".into(),
            pid: Some(42),
            start_id: Some(123),
            ppid: None,
            name: "worker".into(),
            args: None,
            cpu_percent: 1.0,
            cpu_time_seconds: Some(2.0),
            memory_bytes: 1024,
        }
    }

    fn identity(row: &ResourceUsage) -> ResourceIdentity {
        ResourceIdentity::observed(&scope("session/namespace-1"), &observation("sample-1"), row)
    }

    #[test]
    fn process_id_survives_sampling_and_display_changes_but_not_pid_reuse() {
        let row = process();
        let original = identity(&row);
        let mut updated = row.clone();
        updated.name = "renamed".into();
        updated.cpu_percent = 90.0;
        updated.memory_bytes = 8192;
        let later = ResourceIdentity::observed(
            &scope("session/namespace-1"),
            &observation("sample-2"),
            &updated,
        );
        assert_eq!(original.resource_id(), later.resource_id());
        assert_eq!(
            original.matches_process_observation(&scope("session/namespace-1"), &updated),
            Ok(())
        );
        updated.start_id = Some(124);
        assert_ne!(original.resource_id(), identity(&updated).resource_id());
        assert_eq!(
            original.matches_process_observation(&scope("session/namespace-1"), &updated),
            Err(IdentityError::StaleResource)
        );
    }

    #[test]
    fn scopes_sources_environments_and_kinds_do_not_alias() {
        let row = process();
        let id = identity(&row).resource_id();
        let mut variants = Vec::new();
        for environment in [
            EnvironmentKind::Windows,
            EnvironmentKind::Docker,
            EnvironmentKind::WslContainer,
        ] {
            let mut other = row.clone();
            other.environment = environment;
            variants.push(other);
        }
        for source in [None, Some("Debian".into()), Some("Ubuntu:42".into())] {
            let mut other = row.clone();
            other.source = source;
            variants.push(other);
        }
        let mut infra = row.clone();
        infra.kind = ResourceKind::Infra;
        variants.push(infra);
        for other in variants {
            assert_ne!(id, identity(&other).resource_id());
            assert_eq!(
                identity(&row).matches_process_observation(&scope("session/namespace-1"), &other),
                Err(IdentityError::StaleResource)
            );
        }
        let restarted = ResourceIdentity::observed(
            &scope("session/namespace-2"),
            &observation("sample-1"),
            &row,
        );
        assert_ne!(id, restarted.resource_id());
        assert_eq!(
            identity(&row).matches_process_observation(&scope("session/namespace-2"), &row),
            Err(IdentityError::StaleResource)
        );
    }

    #[test]
    fn missing_generation_and_aggregates_are_observation_scoped() {
        for kind in [
            ResourceKind::Process,
            ResourceKind::Infra,
            ResourceKind::Application,
            ResourceKind::Container,
            ResourceKind::Host,
        ] {
            let mut row = process();
            row.kind = kind;
            row.start_id = None;
            let first = identity(&row);
            let next = ResourceIdentity::observed(
                &scope("session/namespace-1"),
                &observation("sample-2"),
                &row,
            );
            assert_ne!(first.resource_id(), next.resource_id());
            let expected = if matches!(kind, ResourceKind::Process | ResourceKind::Infra) {
                IdentityError::UnverifiableProcess
            } else {
                IdentityError::UnsupportedResource
            };
            assert_eq!(
                first.matches_process_observation(&scope("session/namespace-1"), &row),
                Err(expected)
            );
        }
        let mut row = process();
        let first = identity(&row);
        row.start_id = None;
        assert_eq!(
            first.matches_process_observation(&scope("session/namespace-1"), &row),
            Err(IdentityError::UnverifiableProcess)
        );
        row.pid = None;
        row.start_id = Some(123);
        assert_eq!(
            identity(&row).matches_process_observation(&scope("session/namespace-1"), &row),
            Err(IdentityError::UnverifiableProcess)
        );
    }

    #[test]
    fn lookup_rejects_unknown_and_ambiguous_ids_without_parsing_input() {
        let rows = [process()];
        let index = ResourceIndex::new(&scope("session"), &observation("sample"), &rows);
        let id = index.entries()[0].resource_id.as_str();
        assert_eq!(index.lookup(id).unwrap().resource.pid, Some(42));
        assert_eq!(
            index.lookup("42").err(),
            Some(IdentityError::UnknownResource)
        );
        let duplicates = [process(), process()];
        let index = ResourceIndex::new(&scope("session"), &observation("sample"), &duplicates);
        assert_eq!(
            index.lookup(index.entries()[0].resource_id.as_str()).err(),
            Some(IdentityError::AmbiguousResource)
        );
    }

    #[test]
    fn structured_encoding_preserves_arbitrary_namespace_strings() {
        let mut a = process();
        a.source = Some("a:b".into());
        a.id = "c".into();
        let mut b = process();
        b.source = Some("a".into());
        b.id = "b:c".into();
        assert_ne!(identity(&a).resource_id(), identity(&b).resource_id());
        a.source = Some("日本語\"\\\n".into());
        let id = identity(&a).resource_id();
        assert_eq!(
            serde_json::to_value(&id).unwrap().as_str(),
            Some(id.as_str())
        );
        assert_eq!(
            IdentityScope::new(String::new()),
            Err(IdentityError::EmptyEpoch)
        );
        assert_eq!(
            ObservationId::new(String::new()),
            Err(IdentityError::EmptyEpoch)
        );
        let json = serde_json::to_value(&a).unwrap();
        assert!(json.get("resource_id").is_none());
        assert!(json.get("start_id").is_none());
    }

    #[test]
    fn aggregates_with_process_metadata_still_cannot_match_a_process() {
        for kind in [
            ResourceKind::Application,
            ResourceKind::Host,
            ResourceKind::Container,
        ] {
            let mut row = process();
            row.kind = kind;
            assert_eq!(
                identity(&row).matches_process_observation(&scope("session/namespace-1"), &row),
                Err(IdentityError::UnsupportedResource)
            );
        }
    }

    #[test]
    fn query_index_keeps_resources_hidden_by_display_limits() {
        use crate::attribution::AttributionTree;
        use crate::query::{QuerySource, ResourceQuery};
        let mut hidden = process();
        hidden.id = "43".into();
        hidden.pid = Some(43);
        hidden.cpu_percent = 0.0;
        let source = QuerySource {
            resources: vec![process(), hidden],
            pid_resources: Vec::new(),
            tree: AttributionTree {
                host_logical_cpu_count: 16,
                groups: Vec::new(),
                unmapped_children: Vec::new(),
                docker_groups: Vec::new(),
                wslc_groups: Vec::new(),
                windows_applications: Vec::new(),
            },
        };
        let query = ResourceQuery {
            limit: 1,
            ..Default::default()
        };
        assert_eq!(query.flat(&source.resources)[0].pid, Some(42));
        let index = source.resource_index(&scope("session"), &observation("sample"));
        let hidden_id = index.entries()[1].resource_id.as_str();
        assert_eq!(index.lookup(hidden_id).unwrap().resource.pid, Some(43));
        assert_eq!(
            index.lookup(hidden_id).unwrap().identity,
            index.entries()[1].identity
        );
    }
}
