#[cfg(unix)]
use crate::linux;
use crate::model::Snapshot;
use crate::multiwsl;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::error::Error;

type CollectorError = Box<dyn Error>;

pub(crate) trait ProcessSnapshotCollector: Send {
    fn snapshot(&self) -> Result<Snapshot, CollectorError>;
}

trait RunningDistroDiscovery: Send {
    fn running_distros(&self) -> Result<Vec<String>, CollectorError>;
}

#[cfg(any(windows, test))]
trait DefaultDistroDiscovery {
    fn default_distro(&self) -> Result<Option<String>, CollectorError>;
}

struct WslRunningDistroDiscovery;

impl RunningDistroDiscovery for WslRunningDistroDiscovery {
    fn running_distros(&self) -> Result<Vec<String>, CollectorError> {
        multiwsl::running_distros()
    }
}

#[cfg(windows)]
impl DefaultDistroDiscovery for WslRunningDistroDiscovery {
    fn default_distro(&self) -> Result<Option<String>, CollectorError> {
        multiwsl::default_distro()
    }
}

#[cfg(unix)]
struct LocalLinuxProcCollector;

#[cfg(unix)]
impl ProcessSnapshotCollector for LocalLinuxProcCollector {
    fn snapshot(&self) -> Result<Snapshot, CollectorError> {
        Ok(linux::snapshot()?)
    }
}

struct RemoteWslProcCollector {
    distro: String,
    source: Option<String>,
}

impl RemoteWslProcCollector {
    fn new(distro: String, source: Option<String>) -> Self {
        Self { distro, source }
    }
}

impl ProcessSnapshotCollector for RemoteWslProcCollector {
    fn snapshot(&self) -> Result<Snapshot, CollectorError> {
        multiwsl::snapshot(&self.distro, self.source.as_deref())
    }
}

#[derive(Debug)]
pub(crate) struct CollectedSnapshots {
    pub primary: Snapshot,
    pub additional: Vec<(String, Snapshot)>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct StreamAdditionalSnapshots {
    pub snapshots: Vec<(String, Snapshot)>,
    pub running_sources: BTreeSet<String>,
    pub failures: BTreeMap<String, String>,
}

pub(crate) struct PrimaryStreamCollector {
    collector: Box<dyn ProcessSnapshotCollector>,
}

impl PrimaryStreamCollector {
    pub(crate) fn snapshot(&self) -> Result<Snapshot, String> {
        self.collector.snapshot().map_err(|error| error.to_string())
    }
}

pub(crate) struct AdditionalStreamCollector {
    collectors: Vec<(String, Box<dyn ProcessSnapshotCollector>)>,
    distro_discovery: Box<dyn RunningDistroDiscovery>,
    primary_distro: Option<String>,
}

impl AdditionalStreamCollector {
    pub(crate) fn snapshot(&mut self) -> Result<StreamAdditionalSnapshots, String> {
        self.snapshot_with_factory(|name| {
            Box::new(RemoteWslProcCollector::new(
                name.to_string(),
                Some(name.to_string()),
            ))
        })
    }

    fn snapshot_with_factory(
        &mut self,
        mut make_collector: impl FnMut(&str) -> Box<dyn ProcessSnapshotCollector>,
    ) -> Result<StreamAdditionalSnapshots, String> {
        let running: HashSet<_> = self
            .distro_discovery
            .running_distros()
            .map_err(|error| format!("additional WSL unavailable: {error}"))?
            .into_iter()
            .collect();
        for name in &running {
            let is_primary = self
                .primary_distro
                .as_deref()
                .is_some_and(|primary| primary.eq_ignore_ascii_case(name));
            if !is_primary
                && !self
                    .collectors
                    .iter()
                    .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
            {
                self.collectors.push((name.clone(), make_collector(name)));
            }
        }
        let mut result = StreamAdditionalSnapshots::default();
        for (name, collector) in &self.collectors {
            if self
                .primary_distro
                .as_deref()
                .is_some_and(|primary| primary.eq_ignore_ascii_case(name))
            {
                continue;
            }
            if !running
                .iter()
                .any(|running_name| running_name.eq_ignore_ascii_case(name))
            {
                continue;
            }
            result.running_sources.insert(name.clone());
            match collector.snapshot() {
                Ok(snapshot) => result.snapshots.push((name.clone(), snapshot)),
                Err(error) => {
                    result.failures.insert(name.clone(), error.to_string());
                }
            }
        }
        Ok(result)
    }
}

pub(crate) struct StreamCollectorPlan {
    pub primary: PrimaryStreamCollector,
    pub additional: AdditionalStreamCollector,
    pub warnings: Vec<String>,
}

impl StreamCollectorPlan {
    pub(crate) fn native(requested_distro: Option<&str>) -> Result<Self, CollectorError> {
        // Build only the primary synchronously. On Windows this may still need
        // discovery to select a default/fallback primary. Additional distro
        // discovery belongs to the additional stream worker, even in full mode.
        CollectorPlan::native(requested_distro, true).map(CollectorPlan::into_stream)
    }
}

pub(crate) struct CollectorPlan {
    primary: Box<dyn ProcessSnapshotCollector>,
    primary_distro: Option<String>,
    additional: Vec<(String, Box<dyn ProcessSnapshotCollector>)>,
    distro_discovery: Box<dyn RunningDistroDiscovery>,
    warnings: Vec<String>,
}

impl CollectorPlan {
    pub(crate) fn native(
        requested_distro: Option<&str>,
        wsl_only: bool,
    ) -> Result<Self, CollectorError> {
        #[cfg(unix)]
        {
            if requested_distro.is_some() {
                return Err("--distro is only supported by the Windows-native executable".into());
            }
            Ok(Self::wsl_native(wsl_only))
        }
        #[cfg(windows)]
        {
            Self::windows_native(requested_distro, wsl_only)
        }
    }

    #[cfg(unix)]
    pub(crate) fn wsl_native(wsl_only: bool) -> Self {
        let current = std::env::var("WSL_DISTRO_NAME").ok();
        let mut plan = Self {
            primary: Box::new(LocalLinuxProcCollector),
            primary_distro: current.clone(),
            additional: Vec::new(),
            distro_discovery: Box::new(WslRunningDistroDiscovery),
            warnings: Vec::new(),
        };
        if wsl_only {
            return plan;
        }

        match multiwsl::running_distros() {
            Ok(distros) => {
                plan.additional = distros
                    .into_iter()
                    .filter(|name| {
                        !current
                            .as_deref()
                            .is_some_and(|primary| primary.eq_ignore_ascii_case(name))
                    })
                    .map(|name| {
                        let collector =
                            RemoteWslProcCollector::new(name.clone(), Some(name.clone()));
                        (
                            name,
                            Box::new(collector) as Box<dyn ProcessSnapshotCollector>,
                        )
                    })
                    .collect();
            }
            Err(error) => plan.warnings.push(format!(
                "additional WSL distro discovery unavailable: {error}"
            )),
        }
        plan
    }

    #[cfg(windows)]
    fn windows_native(
        requested_distro: Option<&str>,
        wsl_only: bool,
    ) -> Result<Self, CollectorError> {
        let discovery = WslRunningDistroDiscovery;
        let spec = windows_native_spec(requested_distro, wsl_only, &discovery, &discovery)?;
        let primary = spec.primary;
        let additional = spec
            .additional
            .into_iter()
            .map(|spec| {
                let name = spec.distro.clone();
                (
                    name,
                    Box::new(RemoteWslProcCollector::new(spec.distro, spec.source))
                        as Box<dyn ProcessSnapshotCollector>,
                )
            })
            .collect();
        let primary_distro = primary.distro.clone();
        Ok(Self {
            primary: Box::new(RemoteWslProcCollector::new(primary.distro, primary.source)),
            primary_distro: Some(primary_distro),
            additional,
            distro_discovery: Box::new(discovery),
            warnings: spec.warnings,
        })
    }

    pub(crate) fn capture(&self) -> Result<CollectedSnapshots, CollectorError> {
        let primary = self.primary.snapshot()?;
        let mut additional = Vec::new();
        let mut warnings = self.warnings.clone();
        let running: HashSet<_> = if self.additional.is_empty() {
            HashSet::new()
        } else {
            match self.distro_discovery.running_distros() {
                Ok(distros) => distros.into_iter().collect(),
                Err(error) => {
                    warnings.push(format!(
                        "additional WSL distro discovery unavailable: {error}"
                    ));
                    return Ok(CollectedSnapshots {
                        primary,
                        additional,
                        warnings,
                    });
                }
            }
        };
        for (name, collector) in &self.additional {
            if !running
                .iter()
                .any(|running_name| running_name.eq_ignore_ascii_case(name))
            {
                warnings.push(format!(
                    "additional WSL {name} unavailable: distribution is no longer running"
                ));
                continue;
            }
            match collector.snapshot() {
                Ok(snapshot) => additional.push((name.clone(), snapshot)),
                Err(error) => warnings.push(format!("additional WSL {name} unavailable: {error}")),
            }
        }
        Ok(CollectedSnapshots {
            primary,
            additional,
            warnings,
        })
    }

    pub(crate) fn into_stream(self) -> StreamCollectorPlan {
        let mut warnings = self.warnings;
        warnings
            .retain(|warning| !warning.starts_with("additional WSL distro discovery unavailable:"));
        StreamCollectorPlan {
            primary: PrimaryStreamCollector {
                collector: self.primary,
            },
            additional: AdditionalStreamCollector {
                collectors: self.additional,
                distro_discovery: self.distro_discovery,
                primary_distro: self.primary_distro,
            },
            warnings,
        }
    }
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
struct RemoteCollectorSpec {
    distro: String,
    source: Option<String>,
}

#[cfg(any(windows, test))]
#[derive(Debug, PartialEq, Eq)]
struct WindowsNativeSpec {
    primary: RemoteCollectorSpec,
    additional: Vec<RemoteCollectorSpec>,
    warnings: Vec<String>,
}

#[cfg(any(windows, test))]
fn windows_native_spec(
    requested_distro: Option<&str>,
    wsl_only: bool,
    running_discovery: &dyn RunningDistroDiscovery,
    default_discovery: &dyn DefaultDistroDiscovery,
) -> Result<WindowsNativeSpec, CollectorError> {
    let mut warnings = Vec::new();
    let default = if requested_distro.is_none() {
        match default_discovery.default_distro() {
            Ok(distro) => distro,
            Err(error) => {
                warnings.push(format!("default WSL distro discovery unavailable: {error}"));
                None
            }
        }
    } else {
        None
    };
    let running = if !wsl_only || (requested_distro.is_none() && default.is_none()) {
        match running_discovery.running_distros() {
            Ok(distros) => distros,
            Err(error) => {
                if !wsl_only {
                    warnings.push(format!(
                        "additional WSL distro discovery unavailable: {error}"
                    ));
                } else if requested_distro.is_none() {
                    warnings.push(format!(
                        "running WSL distro fallback discovery unavailable: {error}"
                    ));
                }
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let primary = select_primary_distro(requested_distro, default.as_deref(), &running, &warnings)?;
    let additional = if wsl_only {
        Vec::new()
    } else {
        running
            .into_iter()
            .filter(|name| !name.eq_ignore_ascii_case(&primary))
            .map(|distro| RemoteCollectorSpec {
                source: Some(distro.clone()),
                distro,
            })
            .collect()
    };
    Ok(WindowsNativeSpec {
        primary: RemoteCollectorSpec {
            distro: primary,
            source: None,
        },
        additional,
        warnings,
    })
}

#[cfg(any(windows, test))]
fn select_primary_distro(
    requested: Option<&str>,
    default: Option<&str>,
    running: &[String],
    discovery_failures: &[String],
) -> Result<String, CollectorError> {
    requested
        .filter(|name| !name.trim().is_empty())
        .or(default)
        .map(str::to_string)
        .or_else(|| running.first().cloned())
        .ok_or_else(|| {
            let mut message =
                "no WSL distribution is available; install one or pass --distro NAME".to_string();
            if !discovery_failures.is_empty() {
                message.push_str("; discovery failures: ");
                message.push_str(&discovery_failures.join("; "));
            }
            message.into()
        })
}

#[cfg(test)]
mod tests {
    use super::{
        select_primary_distro, windows_native_spec, CollectorError, CollectorPlan,
        DefaultDistroDiscovery, ProcessSnapshotCollector, RemoteCollectorSpec,
        RunningDistroDiscovery,
    };
    use crate::model::Snapshot;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use std::time::Instant;

    struct StubCollector(Result<Snapshot, &'static str>);

    impl ProcessSnapshotCollector for StubCollector {
        fn snapshot(&self) -> Result<Snapshot, CollectorError> {
            match &self.0 {
                Ok(snapshot) => Ok(snapshot.clone()),
                Err(error) => Err((*error).into()),
            }
        }
    }

    struct StubDiscovery(Result<Vec<String>, &'static str>);

    struct ForbiddenDiscovery;

    impl RunningDistroDiscovery for ForbiddenDiscovery {
        fn running_distros(&self) -> Result<Vec<String>, CollectorError> {
            panic!("optional running-distro discovery must be deferred");
        }
    }

    impl DefaultDistroDiscovery for ForbiddenDiscovery {
        fn default_distro(&self) -> Result<Option<String>, CollectorError> {
            panic!("explicit primary must not require default discovery");
        }
    }

    impl RunningDistroDiscovery for StubDiscovery {
        fn running_distros(&self) -> Result<Vec<String>, CollectorError> {
            match &self.0 {
                Ok(distros) => Ok(distros.clone()),
                Err(error) => Err((*error).into()),
            }
        }
    }

    struct StubDefaultDiscovery(Result<Option<String>, &'static str>);

    impl DefaultDistroDiscovery for StubDefaultDiscovery {
        fn default_distro(&self) -> Result<Option<String>, CollectorError> {
            match &self.0 {
                Ok(distro) => Ok(distro.clone()),
                Err(error) => Err((*error).into()),
            }
        }
    }

    struct CountingCollector(Arc<AtomicUsize>);

    #[test]
    fn stream_discovers_and_samples_a_new_distro_without_sampling_primary() {
        struct ChangingDiscovery(AtomicUsize);

        impl RunningDistroDiscovery for ChangingDiscovery {
            fn running_distros(&self) -> Result<Vec<String>, CollectorError> {
                let pass = self.0.fetch_add(1, Ordering::Relaxed);
                Ok(match pass {
                    0 => vec!["UBUNTU".into()],
                    1 => vec!["ubuntu".into(), "Debian".into()],
                    _ => vec!["Ubuntu".into(), "DEBIAN".into()],
                })
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        let mut created = Vec::new();
        let mut make_collector = |name: &str| -> Box<dyn ProcessSnapshotCollector> {
            created.push(name.to_string());
            Box::new(CountingCollector(Arc::clone(&calls)))
        };
        let mut collector = super::AdditionalStreamCollector {
            collectors: Vec::new(),
            distro_discovery: Box::new(ChangingDiscovery(AtomicUsize::new(0))),
            primary_distro: Some("Ubuntu".into()),
        };

        let initial = collector
            .snapshot_with_factory(&mut make_collector)
            .unwrap();
        assert!(initial.running_sources.is_empty());
        assert!(initial.snapshots.is_empty());
        assert!(collector.collectors.is_empty());
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        for expected_calls in 1..=2 {
            let update = collector
                .snapshot_with_factory(&mut make_collector)
                .unwrap();
            assert_eq!(collector.collectors.len(), 1);
            assert_eq!(update.running_sources, ["Debian".into()].into());
            assert_eq!(update.snapshots.len(), 1);
            assert_eq!(update.snapshots[0].0, "Debian");
            assert!(update.failures.is_empty());
            assert_eq!(calls.load(Ordering::Relaxed), expected_calls);
        }
        assert_eq!(created, ["Debian"]);
    }

    impl ProcessSnapshotCollector for CountingCollector {
        fn snapshot(&self) -> Result<Snapshot, CollectorError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(snapshot())
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            captured_at: Instant::now(),
            processes: Vec::new(),
        }
    }

    #[test]
    fn required_collector_failure_is_an_error() {
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Err("primary failed"))),
            primary_distro: None,
            additional: Vec::new(),
            distro_discovery: Box::new(StubDiscovery(Ok(Vec::new()))),
            warnings: Vec::new(),
        };
        assert_eq!(plan.capture().unwrap_err().to_string(), "primary failed");
    }

    #[test]
    fn optional_collector_failure_is_a_warning() {
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            primary_distro: None,
            additional: vec![(
                "Ubuntu-2".to_string(),
                Box::new(StubCollector(Err("remote failed"))),
            )],
            distro_discovery: Box::new(StubDiscovery(Ok(vec!["Ubuntu-2".to_string()]))),
            warnings: Vec::new(),
        };
        let result = plan.capture().unwrap();
        assert!(result.additional.is_empty());
        assert_eq!(
            result.warnings,
            ["additional WSL Ubuntu-2 unavailable: remote failed"]
        );
    }

    #[test]
    fn stopped_optional_distro_is_not_started_by_snapshot_collection() {
        let calls = Arc::new(AtomicUsize::new(0));
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            primary_distro: None,
            additional: vec![(
                "Ubuntu-2".to_string(),
                Box::new(CountingCollector(Arc::clone(&calls))),
            )],
            distro_discovery: Box::new(StubDiscovery(Ok(Vec::new()))),
            warnings: Vec::new(),
        };

        let result = plan.capture().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 0);
        assert!(result.additional.is_empty());
        assert_eq!(
            result.warnings,
            ["additional WSL Ubuntu-2 unavailable: distribution is no longer running"]
        );
    }

    #[test]
    fn capture_running_membership_is_case_insensitive() {
        let calls = Arc::new(AtomicUsize::new(0));
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            primary_distro: None,
            additional: vec![(
                "Ubuntu-2".to_string(),
                Box::new(CountingCollector(Arc::clone(&calls))),
            )],
            distro_discovery: Box::new(StubDiscovery(Ok(vec!["ubuntu-2".to_string()]))),
            warnings: Vec::new(),
        };

        let result = plan.capture().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(result.additional.len(), 1);
        assert_eq!(result.additional[0].0, "Ubuntu-2");
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn stream_plan_reports_each_additional_source_state() {
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            primary_distro: None,
            additional: vec![
                (
                    "running".to_string(),
                    Box::new(StubCollector(Ok(snapshot()))),
                ),
                (
                    "failed".to_string(),
                    Box::new(StubCollector(Err("remote failed"))),
                ),
                (
                    "stopped".to_string(),
                    Box::new(StubCollector(Err("must not be sampled"))),
                ),
            ],
            distro_discovery: Box::new(StubDiscovery(Ok(vec![
                "running".to_string(),
                "failed".to_string(),
            ]))),
            warnings: vec!["initial warning".to_string()],
        };

        let mut stream = plan.into_stream();
        assert_eq!(stream.warnings, ["initial warning"]);
        assert!(stream.primary.snapshot().is_ok());
        let additional = stream.additional.snapshot().unwrap();
        assert_eq!(
            additional.running_sources,
            ["failed".to_string(), "running".to_string()].into()
        );
        assert_eq!(additional.snapshots.len(), 1);
        assert_eq!(additional.snapshots[0].0, "running");
        assert_eq!(
            additional.failures.get("failed").map(String::as_str),
            Some("remote failed")
        );
        assert!(!additional.failures.contains_key("stopped"));
    }

    #[test]
    fn stream_additional_discovery_failure_does_not_block_primary() {
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            primary_distro: None,
            additional: vec![(
                "Ubuntu-2".to_string(),
                Box::new(StubCollector(Ok(snapshot()))),
            )],
            distro_discovery: Box::new(StubDiscovery(Err("discovery failed"))),
            warnings: Vec::new(),
        };

        let mut stream = plan.into_stream();
        assert!(stream.primary.snapshot().is_ok());
        assert_eq!(
            stream.additional.snapshot().unwrap_err(),
            "additional WSL unavailable: discovery failed"
        );
    }

    #[test]
    fn stream_running_membership_is_case_insensitive() {
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            primary_distro: None,
            additional: vec![(
                "Ubuntu-2".to_string(),
                Box::new(StubCollector(Ok(snapshot()))),
            )],
            distro_discovery: Box::new(StubDiscovery(Ok(vec!["ubuntu-2".to_string()]))),
            warnings: Vec::new(),
        };

        let mut stream = plan.into_stream();
        let additional = stream.additional.snapshot().unwrap();
        assert_eq!(additional.snapshots[0].0, "Ubuntu-2");
        assert_eq!(additional.running_sources, ["Ubuntu-2".to_string()].into());
    }

    #[test]
    fn primary_distro_selection_prefers_requested_then_default_then_running() {
        let running = vec!["Running".to_string()];
        assert_eq!(
            select_primary_distro(Some("Requested"), Some("Default"), &running, &[]).unwrap(),
            "Requested"
        );
        assert_eq!(
            select_primary_distro(None, Some("Default"), &running, &[]).unwrap(),
            "Default"
        );
        assert_eq!(
            select_primary_distro(None, None, &running, &[]).unwrap(),
            "Running"
        );
        assert!(select_primary_distro(None, None, &[], &[]).is_err());
    }

    #[test]
    fn primary_distro_selection_preserves_discovery_failures() {
        let failures = vec![
            "additional WSL distro discovery unavailable: wsl service failed".to_string(),
            "default WSL distro discovery unavailable: default query failed".to_string(),
        ];

        let error = select_primary_distro(None, None, &[], &failures).unwrap_err();

        assert_eq!(
            error.to_string(),
            "no WSL distribution is available; install one or pass --distro NAME; discovery failures: additional WSL distro discovery unavailable: wsl service failed; default WSL distro discovery unavailable: default query failed"
        );
    }

    #[test]
    fn stream_never_samples_a_preseeded_primary_as_additional() {
        let calls = Arc::new(AtomicUsize::new(0));
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            primary_distro: Some("Ubuntu-2".to_string()),
            additional: vec![(
                "ubuntu-2".to_string(),
                Box::new(CountingCollector(Arc::clone(&calls))),
            )],
            distro_discovery: Box::new(StubDiscovery(Ok(vec!["UBUNTU-2".to_string()]))),
            warnings: Vec::new(),
        };

        let mut stream = plan.into_stream();
        let additional = stream.additional.snapshot().unwrap();
        assert!(additional.snapshots.is_empty());
        assert!(additional.running_sources.is_empty());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn windows_native_spec_builds_primary_and_additional_collectors() {
        let running = StubDiscovery(Ok(vec!["Ubuntu".to_string(), "Debian".to_string()]));
        let default = StubDefaultDiscovery(Ok(Some("Ubuntu".to_string())));

        let spec = windows_native_spec(None, false, &running, &default).unwrap();

        assert_eq!(
            spec.primary,
            RemoteCollectorSpec {
                distro: "Ubuntu".to_string(),
                source: None,
            }
        );
        assert_eq!(
            spec.additional,
            [RemoteCollectorSpec {
                distro: "Debian".to_string(),
                source: Some("Debian".to_string()),
            }]
        );
        assert!(spec.warnings.is_empty());
    }

    #[test]
    fn windows_native_spec_wsl_only_uses_default_without_additional_collectors() {
        let running = ForbiddenDiscovery;
        let default = StubDefaultDiscovery(Ok(Some("Ubuntu".to_string())));

        let spec = windows_native_spec(None, true, &running, &default).unwrap();

        assert_eq!(spec.primary.distro, "Ubuntu");
        assert_eq!(spec.primary.source, None);
        assert!(spec.additional.is_empty());
        assert!(spec.warnings.is_empty());
    }

    #[test]
    fn stream_primary_selection_skips_discovery_for_explicit_windows_distro() {
        let spec = windows_native_spec(
            Some("Ubuntu"),
            true,
            &ForbiddenDiscovery,
            &ForbiddenDiscovery,
        )
        .unwrap();

        assert_eq!(spec.primary.distro, "Ubuntu");
        assert!(spec.additional.is_empty());
        assert!(spec.warnings.is_empty());
    }

    #[test]
    fn stream_primary_selection_preserves_windows_running_fallback() {
        let running = StubDiscovery(Ok(vec!["Debian".to_string(), "Ubuntu".to_string()]));
        let default = StubDefaultDiscovery(Ok(None));
        let spec = windows_native_spec(None, true, &running, &default).unwrap();

        assert_eq!(spec.primary.distro, "Debian");
        assert!(spec.additional.is_empty());
        assert!(spec.warnings.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn native_stream_starts_with_only_the_local_primary() {
        let plan = super::StreamCollectorPlan::native(None).unwrap();
        assert!(plan.additional.collectors.is_empty());
        assert!(plan.warnings.is_empty());
        assert!(plan.primary.snapshot().is_ok());
    }

    #[test]
    fn windows_native_spec_preserves_injected_discovery_failures() {
        let running = StubDiscovery(Err("running query failed"));
        let default = StubDefaultDiscovery(Err("default query failed"));

        let error = windows_native_spec(None, false, &running, &default).unwrap_err();

        assert!(error.to_string().contains("running query failed"));
        assert!(error.to_string().contains("default query failed"));
    }

    #[test]
    fn windows_native_wsl_only_preserves_running_fallback_failure() {
        let running = StubDiscovery(Err("running fallback failed"));
        let default = StubDefaultDiscovery(Ok(None));

        let error = windows_native_spec(None, true, &running, &default).unwrap_err();

        assert!(error.to_string().contains(
            "running WSL distro fallback discovery unavailable: running fallback failed"
        ));
    }
}
