//! Immutable retained observations for the forthcoming read-only Query API.
//! Reads never collect, refresh, or silently substitute another snapshot.

use crate::identity::{IdentityScope, ObservationId, ResourceIdentity, ResourceIndex};
use crate::model::ResourceUsage;
use crate::monitor::MonitorSnapshot;
use serde::Serialize;
use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SnapshotId(String);

impl SnapshotId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Timing must describe a completed collection, not the later cache insertion.
pub struct SampleTiming {
    started: Instant,
    captured: Instant,
    captured_at: SystemTime,
}

impl SampleTiming {
    pub fn new(
        started: Instant,
        captured: Instant,
        captured_at: SystemTime,
    ) -> Result<Self, SnapshotError> {
        if captured < started {
            return Err(SnapshotError::InvalidTiming);
        }
        Ok(Self {
            started,
            captured,
            captured_at,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotError {
    InvalidRetention,
    InvalidTiming,
    MissingQuerySource,
    SnapshotUnavailable,
    TooOld,
    SequenceExhausted,
}

pub enum SnapshotRequest<'a> {
    Latest {
        max_age: Duration,
    },
    /// Retention still applies; max_age is deliberately not part of a pinned read.
    Id(&'a str),
}

pub struct StoredSnapshot {
    id: SnapshotId,
    scope: IdentityScope,
    observation: ObservationId,
    timing: SampleTiming,
    snapshot: MonitorSnapshot,
}

impl StoredSnapshot {
    pub fn resource_identity(&self, row: &ResourceUsage) -> ResourceIdentity {
        ResourceIdentity::observed(&self.scope, &self.observation, row)
    }

    pub fn id(&self) -> &SnapshotId {
        &self.id
    }
    pub fn captured_at(&self) -> SystemTime {
        self.timing.captured_at
    }
    pub fn sample_window(&self) -> Duration {
        self.timing.captured - self.timing.started
    }
    pub fn snapshot(&self) -> &MonitorSnapshot {
        &self.snapshot
    }

    pub fn resource_index(&self) -> ResourceIndex<'_> {
        self.snapshot
            .query_source
            .as_ref()
            .expect("store accepts only complete query sources")
            .resource_index(&self.scope, &self.observation)
    }
}

/// Bounded by count and age. The caller supplies a service-session epoch that
/// must never be reused by another store/session. Namespace restarts must be
/// signalled explicitly; this cache does not detect them or collect live state.
pub struct SnapshotStore {
    service: IdentityScope,
    namespace: u64,
    sequence: u64,
    capacity: NonZeroUsize,
    retention: Duration,
    last_capture: Option<Instant>,
    entries: VecDeque<StoredSnapshot>,
}

impl SnapshotStore {
    pub fn new(
        service: IdentityScope,
        capacity: NonZeroUsize,
        retention: Duration,
    ) -> Result<Self, SnapshotError> {
        if retention.is_zero() {
            return Err(SnapshotError::InvalidRetention);
        }
        Ok(Self {
            service,
            namespace: 0,
            sequence: 0,
            capacity,
            retention,
            last_capture: None,
            entries: VecDeque::new(),
        })
    }

    /// Invalidate old observations and generation identities before sampling a
    /// changed namespace/configuration. Counters never reset or wrap.
    pub fn rotate_namespace(&mut self) -> Result<(), SnapshotError> {
        self.namespace = self
            .namespace
            .checked_add(1)
            .ok_or(SnapshotError::SequenceExhausted)?;
        self.entries.clear();
        self.last_capture = None;
        Ok(())
    }

    pub fn insert(
        &mut self,
        snapshot: MonitorSnapshot,
        timing: SampleTiming,
    ) -> Result<SnapshotId, SnapshotError> {
        self.insert_at(snapshot, timing, Instant::now())
    }

    fn insert_at(
        &mut self,
        snapshot: MonitorSnapshot,
        timing: SampleTiming,
        now: Instant,
    ) -> Result<SnapshotId, SnapshotError> {
        if snapshot.query_source.is_none() {
            return Err(SnapshotError::MissingQuerySource);
        }
        if timing.captured > now || self.last_capture.is_some_and(|last| timing.captured < last) {
            return Err(SnapshotError::InvalidTiming);
        }
        if now.duration_since(timing.captured) >= self.retention {
            return Err(SnapshotError::TooOld);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(SnapshotError::SequenceExhausted)?;
        // Structured encodings avoid delimiter collisions. These strings are
        // private opaque handles, not authorization credentials.
        let id = SnapshotId(
            serde_json::to_string(&(&self.service, sequence)).expect("scope and counter serialize"),
        );
        let scope = IdentityScope::new(
            serde_json::to_string(&(&self.service, self.namespace))
                .expect("scope and counter serialize"),
        )
        .expect("encoded scope is nonempty");
        let observation = ObservationId::new(id.0.clone()).expect("encoded ID is nonempty");
        self.entries
            .retain(|entry| now.duration_since(entry.timing.captured) < self.retention);
        while self.entries.len() >= self.capacity.get() {
            self.entries.pop_front();
        }
        self.sequence = sequence;
        self.last_capture = Some(timing.captured);
        self.entries.push_back(StoredSnapshot {
            id: id.clone(),
            scope,
            observation,
            timing,
            snapshot,
        });
        Ok(id)
    }

    pub fn get(&self, request: SnapshotRequest<'_>) -> Result<&StoredSnapshot, SnapshotError> {
        self.get_at(request, Instant::now())
    }

    fn get_at(
        &self,
        request: SnapshotRequest<'_>,
        now: Instant,
    ) -> Result<&StoredSnapshot, SnapshotError> {
        let entry = match request {
            SnapshotRequest::Latest { .. } => self.entries.back(),
            SnapshotRequest::Id(id) => self.entries.iter().find(|entry| entry.id.as_str() == id),
        }
        .ok_or(SnapshotError::SnapshotUnavailable)?;
        let age = now
            .checked_duration_since(entry.timing.captured)
            .ok_or(SnapshotError::InvalidTiming)?;
        if age >= self.retention {
            return Err(SnapshotError::SnapshotUnavailable);
        }
        if let SnapshotRequest::Latest { max_age } = request {
            if age > max_age {
                return Err(SnapshotError::TooOld);
            }
        }
        Ok(entry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attribution::AttributionTree;
    use crate::model::ResourceUsage;
    use crate::query::{QuerySource, Sort};

    fn store(capacity: usize) -> SnapshotStore {
        SnapshotStore::new(
            IdentityScope::new("service-1".into()).unwrap(),
            NonZeroUsize::new(capacity).unwrap(),
            Duration::from_secs(10),
        )
        .unwrap()
    }

    fn snapshot(cpu: f64) -> MonitorSnapshot {
        let rows: Vec<ResourceUsage> = vec![crate::query::tests::row("worker", cpu, 1024)];
        let tree = AttributionTree {
            host_logical_cpu_count: 16,
            groups: vec![],
            unmapped_children: vec![],
            docker_groups: vec![],
            wslc_groups: vec![],
            windows_applications: vec![],
        };
        MonitorSnapshot {
            host_cpu_percent: Some(cpu),
            host_memory: None,
            host_history: Default::default(),
            environment_summary: Default::default(),
            sort: Sort::default(),
            query_source: Some(QuerySource {
                resources: rows.clone(),
                pid_resources: rows.clone(),
                tree: tree.clone(),
            }),
            host_logical_cpu_count: 16,
            resources: vec![],
            pid_resources: vec![],
            tree,
            warnings: vec!["optional collector unavailable".into()],
        }
    }

    fn timing(at: Instant) -> SampleTiming {
        SampleTiming::new(
            at - Duration::from_secs(3),
            at,
            SystemTime::UNIX_EPOCH + Duration::from_secs(100),
        )
        .unwrap()
    }

    #[test]
    fn pinned_reads_preserve_observation_while_latest_advances() {
        let now = Instant::now();
        let later = now + Duration::from_secs(1);
        let mut cache = store(3);
        let first = cache.insert_at(snapshot(1.0), timing(now), now).unwrap();
        let second = cache
            .insert_at(snapshot(2.0), timing(later), later)
            .unwrap();
        assert_ne!(first, second);
        let pinned = cache
            .get_at(SnapshotRequest::Id(first.as_str()), later)
            .unwrap();
        assert_eq!(pinned.snapshot().host_cpu_percent, Some(1.0));
        assert_eq!(
            pinned.snapshot().warnings,
            ["optional collector unavailable"]
        );
        assert_eq!(pinned.sample_window(), Duration::from_secs(3));
        assert_eq!(
            pinned.captured_at(),
            SystemTime::UNIX_EPOCH + Duration::from_secs(100)
        );
        assert_eq!(pinned.resource_index().entries().len(), 1);
        assert!(pinned.snapshot().resources.is_empty()); // Display limit is irrelevant.
        assert_eq!(
            cache
                .get_at(
                    SnapshotRequest::Latest {
                        max_age: Duration::ZERO
                    },
                    later
                )
                .unwrap()
                .id(),
            &second
        );
        assert_eq!(
            cache
                .get_at(
                    SnapshotRequest::Latest {
                        max_age: Duration::ZERO
                    },
                    later + Duration::from_millis(1)
                )
                .err(),
            Some(SnapshotError::TooOld)
        );
        assert_eq!(
            cache
                .get_at(SnapshotRequest::Id(first.as_str()), later)
                .unwrap()
                .id(),
            &first
        );
    }

    #[test]
    fn retention_and_capacity_never_fall_back_to_latest() {
        let now = Instant::now();
        let mut cache = store(1);
        let first = cache.insert_at(snapshot(1.0), timing(now), now).unwrap();
        let later = now + Duration::from_secs(1);
        let second = cache
            .insert_at(snapshot(2.0), timing(later), later)
            .unwrap();
        assert_eq!(
            cache
                .get_at(SnapshotRequest::Id(first.as_str()), later)
                .err(),
            Some(SnapshotError::SnapshotUnavailable)
        );
        assert_eq!(
            cache.get_at(SnapshotRequest::Id("not-an-id"), later).err(),
            Some(SnapshotError::SnapshotUnavailable)
        );
        assert!(cache
            .get_at(
                SnapshotRequest::Id(second.as_str()),
                later + Duration::from_secs(9)
            )
            .is_ok());
        assert_eq!(
            cache
                .get_at(
                    SnapshotRequest::Id(second.as_str()),
                    later + Duration::from_secs(10)
                )
                .err(),
            Some(SnapshotError::SnapshotUnavailable)
        );
        assert_eq!(
            cache
                .get_at(
                    SnapshotRequest::Latest {
                        max_age: Duration::MAX
                    },
                    later + Duration::from_secs(10)
                )
                .err(),
            Some(SnapshotError::SnapshotUnavailable)
        );
    }

    #[test]
    fn identities_follow_generation_observation_and_namespace_lifetimes() {
        let now = Instant::now();
        let mut cache = store(4);
        let first = cache.insert_at(snapshot(1.0), timing(now), now).unwrap();
        let resource_id = cache
            .get_at(SnapshotRequest::Id(first.as_str()), now)
            .unwrap()
            .resource_index()
            .entries()[0]
            .resource_id
            .clone();
        let second = cache.insert_at(snapshot(2.0), timing(now), now).unwrap();
        assert_eq!(
            cache
                .get_at(SnapshotRequest::Id(second.as_str()), now)
                .unwrap()
                .resource_index()
                .entries()[0]
                .resource_id,
            resource_id
        );
        let mut unknown = snapshot(1.0);
        unknown.query_source.as_mut().unwrap().resources[0].start_id = None;
        let third = cache.insert_at(unknown, timing(now), now).unwrap();
        let unknown_id = cache
            .get_at(SnapshotRequest::Id(third.as_str()), now)
            .unwrap()
            .resource_index()
            .entries()[0]
            .resource_id
            .clone();
        let mut unknown = snapshot(1.0);
        unknown.query_source.as_mut().unwrap().resources[0].start_id = None;
        let fourth = cache.insert_at(unknown, timing(now), now).unwrap();
        assert_ne!(
            cache
                .get_at(SnapshotRequest::Id(fourth.as_str()), now)
                .unwrap()
                .resource_index()
                .entries()[0]
                .resource_id,
            unknown_id
        );
        cache.rotate_namespace().unwrap();
        assert_eq!(
            cache
                .get_at(SnapshotRequest::Id(second.as_str()), now)
                .err(),
            Some(SnapshotError::SnapshotUnavailable)
        );
        let fifth = cache.insert_at(snapshot(1.0), timing(now), now).unwrap();
        assert_ne!(first, fifth);
        assert_ne!(
            cache
                .get_at(SnapshotRequest::Id(fifth.as_str()), now)
                .unwrap()
                .resource_index()
                .entries()[0]
                .resource_id,
            resource_id
        );
    }

    #[test]
    fn rejected_insertions_preserve_the_last_good_snapshot() {
        let now = Instant::now();
        let mut cache = store(1);
        assert_eq!(
            cache
                .get_at(
                    SnapshotRequest::Latest {
                        max_age: Duration::MAX
                    },
                    now
                )
                .err(),
            Some(SnapshotError::SnapshotUnavailable)
        );
        let id = cache.insert_at(snapshot(1.0), timing(now), now).unwrap();
        let mut incomplete = snapshot(2.0);
        incomplete.query_source = None;
        assert_eq!(
            cache.insert_at(incomplete, timing(now), now),
            Err(SnapshotError::MissingQuerySource)
        );
        assert_eq!(
            cache.insert_at(snapshot(2.0), timing(now + Duration::from_secs(1)), now),
            Err(SnapshotError::InvalidTiming)
        );
        assert_eq!(
            cache.insert_at(snapshot(2.0), timing(now - Duration::from_secs(1)), now),
            Err(SnapshotError::InvalidTiming)
        );
        assert_eq!(
            cache.insert_at(snapshot(2.0), timing(now), now + Duration::from_secs(10)),
            Err(SnapshotError::TooOld)
        );
        assert_eq!(
            cache
                .get_at(SnapshotRequest::Id(id.as_str()), now)
                .unwrap()
                .snapshot()
                .host_cpu_percent,
            Some(1.0)
        );
        cache.sequence = u64::MAX;
        assert_eq!(
            cache.insert_at(snapshot(2.0), timing(now), now),
            Err(SnapshotError::SequenceExhausted)
        );
        assert_eq!(
            cache
                .get_at(SnapshotRequest::Id(id.as_str()), now)
                .unwrap()
                .id(),
            &id
        );
    }

    #[test]
    fn freshness_uses_capture_time_and_accepts_the_exact_age_limit() {
        let now = Instant::now();
        let inserted = now + Duration::from_secs(2);
        let mut cache = store(2);
        cache
            .insert_at(snapshot(1.0), timing(now), inserted)
            .unwrap();
        assert!(cache
            .get_at(
                SnapshotRequest::Latest {
                    max_age: Duration::from_secs(2)
                },
                inserted
            )
            .is_ok());
        assert_eq!(
            cache
                .get_at(
                    SnapshotRequest::Latest {
                        max_age: Duration::from_secs(1)
                    },
                    inserted
                )
                .err(),
            Some(SnapshotError::TooOld)
        );
        assert_eq!(
            cache
                .get_at(
                    SnapshotRequest::Latest {
                        max_age: Duration::MAX
                    },
                    now - Duration::from_secs(1)
                )
                .err(),
            Some(SnapshotError::InvalidTiming)
        );
    }

    #[test]
    fn configuration_session_separation_and_namespace_exhaustion() {
        let now = Instant::now();
        assert!(matches!(
            SnapshotStore::new(
                IdentityScope::new("service".into()).unwrap(),
                NonZeroUsize::new(1).unwrap(),
                Duration::ZERO
            ),
            Err(SnapshotError::InvalidRetention)
        ));
        assert!(matches!(
            SampleTiming::new(now, now - Duration::from_secs(1), SystemTime::now()),
            Err(SnapshotError::InvalidTiming)
        ));
        let mut first = store(1);
        let mut second = SnapshotStore::new(
            IdentityScope::new("service-2".into()).unwrap(),
            NonZeroUsize::new(1).unwrap(),
            Duration::from_secs(10),
        )
        .unwrap();
        let a = first.insert_at(snapshot(1.0), timing(now), now).unwrap();
        let b = second.insert_at(snapshot(1.0), timing(now), now).unwrap();
        assert_ne!(a, b);
        assert_eq!(
            second.get_at(SnapshotRequest::Id(a.as_str()), now).err(),
            Some(SnapshotError::SnapshotUnavailable)
        );
        first.namespace = u64::MAX;
        assert_eq!(
            first.rotate_namespace(),
            Err(SnapshotError::SequenceExhausted)
        );
        assert!(first.get_at(SnapshotRequest::Id(a.as_str()), now).is_ok());
    }
}
