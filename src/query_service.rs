//! Collection/cache orchestration with independent retained reads for MCP.

use crate::identity::IdentityScope;
use crate::monitor::{Monitor, MonitorSnapshot};
use crate::query_api::{QueryError, QueryView};
use crate::snapshot_store::{
    IdentityLifetime, InsertError, SampleTiming, SnapshotError, SnapshotId, SnapshotRequest,
    SnapshotStore,
};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// Own the collector so reconfiguration goes through replace_collector. A
/// collection must finish (including worker joins) before returning its snapshot.
pub trait SnapshotCollector {
    fn collect(&mut self) -> Result<MonitorSnapshot, String>;
}

impl SnapshotCollector for Monitor {
    fn collect(&mut self) -> Result<MonitorSnapshot, String> {
        self.sample().map_err(|error| error.to_string())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServiceError {
    LockPoisoned,
    Entropy(String),
    Collection(String),
    Snapshot(SnapshotError),
    Query(QueryError),
}

pub struct QueryService<C> {
    collector: C,
    store: SnapshotStore,
}

impl<C: SnapshotCollector> QueryService<C> {
    pub fn into_shared(self) -> SharedQueryService<C> {
        SharedQueryService {
            collector: Mutex::new(self.collector),
            store: Mutex::new(self.store),
        }
    }
    pub fn new(
        collector: C,
        capacity: NonZeroUsize,
        retention: Duration,
    ) -> Result<Self, ServiceError> {
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|error| ServiceError::Entropy(error.to_string()))?;
        let epoch = nonce
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let scope = IdentityScope::new(epoch).expect("256-bit session epoch is nonempty");
        let store =
            SnapshotStore::new(scope, capacity, retention).map_err(ServiceError::Snapshot)?;
        Ok(Self { collector, store })
    }

    pub fn query(&mut self, request: SnapshotRequest<'_>) -> Result<QueryView<'_>, ServiceError> {
        let id = match request {
            SnapshotRequest::Id(id) => {
                return QueryView::open(&self.store, SnapshotRequest::Id(id))
                    .map_err(ServiceError::Query);
            }
            SnapshotRequest::Latest { max_age } if max_age.is_zero() => self.refresh()?,
            SnapshotRequest::Latest { max_age } => {
                match self.store.get(SnapshotRequest::Latest { max_age }) {
                    Ok(snapshot) => snapshot.id().clone(),
                    Err(SnapshotError::TooOld | SnapshotError::SnapshotUnavailable) => {
                        self.refresh()?
                    }
                    Err(error) => return Err(ServiceError::Snapshot(error)),
                }
            }
        };
        // A refresh is fulfilled by its newly completed observation. In
        // particular max_age=0 means refresh, not an impossible zero-time read.
        QueryView::open(&self.store, SnapshotRequest::Id(id.as_str())).map_err(ServiceError::Query)
    }

    fn refresh(&mut self) -> Result<SnapshotId, ServiceError> {
        let started = Instant::now();
        let snapshot = self.collector.collect().map_err(ServiceError::Collection)?;
        let timing = SampleTiming::new(started, Instant::now(), SystemTime::now())
            .map_err(ServiceError::Snapshot)?;
        self.store
            .insert_checked(
                snapshot,
                timing,
                IdentityLifetime::Observation,
                |snapshot| QueryView::from_snapshot(snapshot).map(|_| ()),
            )
            .map_err(|error| match error {
                InsertError::Snapshot(error) => ServiceError::Snapshot(error),
                InsertError::Validation(error) => ServiceError::Query(error),
            })
    }

    /// Exclusive ownership prevents an old in-flight sample from being inserted
    /// after reconfiguration. Existing borrowed views must be released first.
    pub fn replace_collector(&mut self, collector: C) -> Result<(), ServiceError> {
        self.store
            .rotate_namespace()
            .map_err(ServiceError::Snapshot)?;
        self.collector = collector;
        Ok(())
    }
}

/// Collection owns only the collector lock. Retained reads hold the store lock
/// while building an owned response, and never wait for native collection.
pub struct SharedQueryService<C> {
    collector: Mutex<C>,
    store: Mutex<SnapshotStore>,
}

impl<C: SnapshotCollector> SharedQueryService<C> {
    pub fn query<R>(
        &self,
        request: SnapshotRequest<'_>,
        reply: impl FnOnce(QueryView<'_>) -> R,
    ) -> Result<R, ServiceError> {
        let max_age = match request {
            SnapshotRequest::Id(id) => {
                let store = self.store.lock().map_err(|_| ServiceError::LockPoisoned)?;
                return QueryView::open(&store, SnapshotRequest::Id(id))
                    .map(reply)
                    .map_err(ServiceError::Query);
            }
            SnapshotRequest::Latest { max_age } => max_age,
        };
        let mut collector = self
            .collector
            .lock()
            .map_err(|_| ServiceError::LockPoisoned)?;
        if !max_age.is_zero() {
            let store = self.store.lock().map_err(|_| ServiceError::LockPoisoned)?;
            match QueryView::open(&store, SnapshotRequest::Latest { max_age }) {
                Ok(view) => return Ok(reply(view)),
                Err(QueryError::Snapshot(
                    SnapshotError::TooOld | SnapshotError::SnapshotUnavailable,
                )) => {}
                Err(error) => return Err(ServiceError::Query(error)),
            }
        }
        let started = Instant::now();
        let snapshot = collector.collect().map_err(ServiceError::Collection)?;
        let timing = SampleTiming::new(started, Instant::now(), SystemTime::now())
            .map_err(ServiceError::Snapshot)?;
        let mut store = self.store.lock().map_err(|_| ServiceError::LockPoisoned)?;
        let id = store
            .insert_checked(
                snapshot,
                timing,
                IdentityLifetime::Observation,
                |snapshot| QueryView::from_snapshot(snapshot).map(|_| ()),
            )
            .map_err(|error| match error {
                InsertError::Snapshot(error) => ServiceError::Snapshot(error),
                InsertError::Validation(error) => ServiceError::Query(error),
            })?;
        QueryView::open(&store, SnapshotRequest::Id(id.as_str()))
            .map(reply)
            .map_err(ServiceError::Query)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::IdentityError;
    use crate::query_api::ListOptions;
    use crate::snapshot_store::tests::snapshot;
    use std::collections::VecDeque;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct FakeCollector {
        samples: VecDeque<Result<MonitorSnapshot, String>>,
        calls: Arc<AtomicUsize>,
    }

    impl SnapshotCollector for FakeCollector {
        fn collect(&mut self) -> Result<MonitorSnapshot, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.samples.pop_front().expect("unexpected collection")
        }
    }

    fn collector(
        samples: Vec<Result<MonitorSnapshot, String>>,
    ) -> (FakeCollector, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            FakeCollector {
                samples: samples.into(),
                calls: calls.clone(),
            },
            calls,
        )
    }

    fn service(
        samples: Vec<Result<MonitorSnapshot, String>>,
    ) -> (QueryService<FakeCollector>, Arc<AtomicUsize>) {
        let (collector, calls) = collector(samples);
        (
            QueryService::new(
                collector,
                NonZeroUsize::new(4).unwrap(),
                Duration::from_secs(60),
            )
            .unwrap(),
            calls,
        )
    }

    fn latest() -> SnapshotRequest<'static> {
        SnapshotRequest::Latest {
            max_age: Duration::from_secs(60),
        }
    }

    #[test]
    fn pinned_reads_finish_while_refresh_is_blocked() {
        use std::sync::mpsc;
        struct BlockingCollector {
            started: mpsc::Sender<()>,
            release: mpsc::Receiver<()>,
        }
        impl SnapshotCollector for BlockingCollector {
            fn collect(&mut self) -> Result<MonitorSnapshot, String> {
                self.started.send(()).unwrap();
                self.release.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(snapshot(2.0))
            }
        }
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let (mut original, _) = service(vec![Ok(snapshot(1.0))]);
        let first = original
            .query(latest())
            .unwrap()
            .get_system_summary()
            .snapshot
            .snapshot_id
            .clone();
        let shared = Arc::new(SharedQueryService {
            collector: Mutex::new(BlockingCollector {
                started: started_tx,
                release: release_rx,
            }),
            store: Mutex::new(original.store),
        });
        let worker_service = shared.clone();
        let worker = std::thread::spawn(move || {
            worker_service
                .query(refresh(), |view| {
                    view.get_system_summary().snapshot.snapshot_id.clone()
                })
                .unwrap()
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let (read_tx, read_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            let result = shared.query(SnapshotRequest::Id(first.as_str()), |view| {
                assert_eq!(view.get_system_summary().data.host_cpu_percent, Some(1.0));
                let rows = view.list_resources(&ListOptions::default());
                let id = rows.data[0].resource_id.as_str();
                assert!(view.inspect_resource(id).is_ok());
                assert!(view.list_children(id, &ListOptions::default()).is_ok());
            });
            assert!(matches!(
                shared.query(SnapshotRequest::Id("missing"), |_| ()),
                Err(ServiceError::Query(QueryError::Snapshot(
                    SnapshotError::SnapshotUnavailable
                )))
            ));
            read_tx.send(result).unwrap();
            first
        });
        let read = read_rx.recv_timeout(Duration::from_secs(2));
        release_tx.send(()).unwrap();
        let second = worker.join().unwrap();
        let first = reader.join().unwrap();
        read.expect("pinned reads must not wait for collection")
            .unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn shared_service_preserves_cache_on_failure_and_reuses_fresh_samples() {
        let (service, calls) = service(vec![Ok(snapshot(1.0)), Err("failed".into())]);
        let service = service.into_shared();
        let first = service
            .query(latest(), |view| {
                view.get_system_summary().snapshot.snapshot_id.clone()
            })
            .unwrap();
        assert_eq!(
            service
                .query(latest(), |view| view
                    .get_system_summary()
                    .snapshot
                    .snapshot_id
                    .clone())
                .unwrap(),
            first
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            service.query(refresh(), |_| ()).err(),
            Some(ServiceError::Collection("failed".into()))
        );
        assert!(service
            .query(SnapshotRequest::Id(first.as_str()), |_| ())
            .is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
    fn refresh() -> SnapshotRequest<'static> {
        SnapshotRequest::Latest {
            max_age: Duration::ZERO,
        }
    }

    #[test]
    fn fresh_and_pinned_queries_reuse_collection_but_zero_age_refreshes() {
        let (mut service, calls) = service(vec![Ok(snapshot(1.0)), Ok(snapshot(2.0))]);
        let view = service.query(latest()).unwrap();
        let first = view.get_system_summary().snapshot.snapshot_id.clone();
        let first_resource = view.list_resources(&ListOptions::default()).data[0]
            .resource_id
            .clone();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            service
                .query(latest())
                .unwrap()
                .get_system_summary()
                .snapshot
                .snapshot_id,
            &first
        );
        let pinned = service.query(SnapshotRequest::Id(first.as_str())).unwrap();
        assert!(pinned.inspect_resource(first_resource.as_str()).is_ok());
        assert!(pinned
            .list_children(first_resource.as_str(), &ListOptions::default())
            .unwrap()
            .data
            .is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let next = service.query(refresh()).unwrap();
        let second = next.get_system_summary().snapshot.snapshot_id.clone();
        assert_ne!(first, second);
        assert_eq!(next.get_system_summary().data.host_cpu_percent, Some(2.0));
        // Even known-generation native rows are observation-scoped until the
        // service can verify namespace continuity across collections.
        assert_eq!(
            next.inspect_resource(first_resource.as_str()).err(),
            Some(QueryError::Identity(IdentityError::UnknownResource))
        );
        assert_eq!(
            service
                .query(SnapshotRequest::Id(first.as_str()))
                .unwrap()
                .get_system_summary()
                .data
                .host_cpu_percent,
            Some(1.0)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            service.query(SnapshotRequest::Id("missing")).err(),
            Some(ServiceError::Query(QueryError::Snapshot(
                SnapshotError::SnapshotUnavailable
            )))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failed_collection_and_invalid_catalog_preserve_last_good_at_capacity() {
        let mut invalid = snapshot(3.0);
        let source = invalid.query_source.as_mut().unwrap();
        source.resources.push(source.resources[0].clone());
        let (collector, calls) = collector(vec![
            Ok(snapshot(1.0)),
            Err("collector failed".into()),
            Ok(invalid),
            Ok(snapshot(4.0)),
        ]);
        let mut service = QueryService::new(
            collector,
            NonZeroUsize::new(1).unwrap(),
            Duration::from_secs(60),
        )
        .unwrap();
        let first = service
            .query(latest())
            .unwrap()
            .get_system_summary()
            .snapshot
            .snapshot_id
            .clone();
        assert_eq!(
            service.query(refresh()).err(),
            Some(ServiceError::Collection("collector failed".into()))
        );
        assert_eq!(
            service.query(refresh()).err(),
            Some(ServiceError::Query(QueryError::Identity(
                IdentityError::AmbiguousResource
            )))
        );
        assert_eq!(
            service
                .query(SnapshotRequest::Id(first.as_str()))
                .unwrap()
                .get_system_summary()
                .data
                .host_cpu_percent,
            Some(1.0)
        );
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let good = service.query(refresh()).unwrap();
        assert_eq!(good.get_system_summary().data.host_cpu_percent, Some(4.0));
        assert_eq!(
            service.query(SnapshotRequest::Id(first.as_str())).err(),
            Some(ServiceError::Query(QueryError::Snapshot(
                SnapshotError::SnapshotUnavailable
            )))
        );
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn stale_latest_refreshes_and_records_actual_collection_window() {
        let (mut service, calls) = service(vec![Ok(snapshot(2.0))]);
        let captured = Instant::now() - Duration::from_secs(2);
        let old = service
            .store
            .insert(
                snapshot(1.0),
                SampleTiming::new(
                    captured - Duration::from_secs(3),
                    captured,
                    SystemTime::UNIX_EPOCH,
                )
                .unwrap(),
            )
            .unwrap();
        let started = SystemTime::now();
        let view = service
            .query(SnapshotRequest::Latest {
                max_age: Duration::from_secs(1),
            })
            .unwrap();
        let reply = view.get_system_summary();
        assert_ne!(reply.snapshot.snapshot_id, &old);
        assert!(reply.snapshot.captured_at >= started);
        assert_eq!(reply.snapshot.warnings, ["optional collector unavailable"]);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reconfiguration_invalidates_old_snapshots_without_collecting() {
        let (mut service, old_calls) = service(vec![Ok(snapshot(1.0))]);
        let first = service
            .query(latest())
            .unwrap()
            .get_system_summary()
            .snapshot
            .snapshot_id
            .clone();
        let (replacement, new_calls) = collector(vec![Ok(snapshot(2.0))]);
        service.replace_collector(replacement).unwrap();
        assert_eq!(
            service.query(SnapshotRequest::Id(first.as_str())).err(),
            Some(ServiceError::Query(QueryError::Snapshot(
                SnapshotError::SnapshotUnavailable
            )))
        );
        assert_eq!(new_calls.load(Ordering::SeqCst), 0);
        let second = service
            .query(latest())
            .unwrap()
            .get_system_summary()
            .snapshot
            .snapshot_id
            .clone();
        assert_ne!(first, second);
        assert_eq!(old_calls.load(Ordering::SeqCst), 1);
        assert_eq!(new_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn independent_services_allocate_distinct_sessions() {
        let (mut a, _) = service(vec![Ok(snapshot(1.0))]);
        let (mut b, _) = service(vec![Ok(snapshot(1.0))]);
        let a = a.query(latest()).unwrap();
        let b = b.query(latest()).unwrap();
        assert_ne!(
            a.get_system_summary().snapshot.snapshot_id,
            b.get_system_summary().snapshot.snapshot_id
        );
        assert_ne!(
            a.list_resources(&ListOptions::default()).data[0].resource_id,
            b.list_resources(&ListOptions::default()).data[0].resource_id
        );
    }

    #[cfg(unix)]
    #[test]
    #[ignore = "requires a live WSL2 environment; run explicitly"]
    fn native_monitor_service_smoke() {
        let config = crate::monitor::MonitorConfig {
            sort: Default::default(),
            interval: Duration::from_millis(50),
            limit: 1,
            show_wsl_host: false,
            wsl_only: true,
            no_wslc: true,
            no_docker: true,
            hide_infra: false,
            show_container_processes: true,
            container_process_limit: 5,
            collect_windows_applications: false,
        };
        let monitor = Monitor::new(config, None);
        let mut service = QueryService::new(
            monitor,
            NonZeroUsize::new(2).unwrap(),
            Duration::from_secs(60),
        )
        .unwrap();
        let view = service.query(latest()).unwrap();
        let snapshot_id = view.get_system_summary().snapshot.snapshot_id.clone();
        let rows = view.list_resources(&ListOptions::default());
        assert!(!rows.data.is_empty());
        let resource_id = rows.data[0].resource_id.clone();
        assert!(service
            .query(SnapshotRequest::Id(snapshot_id.as_str()))
            .unwrap()
            .inspect_resource(resource_id.as_str())
            .is_ok());
    }
}
