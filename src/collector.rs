use crate::model::Snapshot;
use crate::{linux, multiwsl};
use std::collections::HashSet;
use std::error::Error;

type CollectorError = Box<dyn Error>;

pub(crate) trait ProcessSnapshotCollector {
    fn snapshot(&self) -> Result<Snapshot, CollectorError>;
}

trait RunningDistroDiscovery {
    fn running_distros(&self) -> Result<Vec<String>, CollectorError>;
}

struct WslRunningDistroDiscovery;

impl RunningDistroDiscovery for WslRunningDistroDiscovery {
    fn running_distros(&self) -> Result<Vec<String>, CollectorError> {
        multiwsl::running_distros()
    }
}

struct LocalLinuxProcCollector;

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

pub(crate) struct CollectorPlan {
    primary: Box<dyn ProcessSnapshotCollector>,
    additional: Vec<(String, Box<dyn ProcessSnapshotCollector>)>,
    distro_discovery: Box<dyn RunningDistroDiscovery>,
    warnings: Vec<String>,
}

impl CollectorPlan {
    pub(crate) fn wsl_native(wsl_only: bool) -> Self {
        let mut plan = Self {
            primary: Box::new(LocalLinuxProcCollector),
            additional: Vec::new(),
            distro_discovery: Box::new(WslRunningDistroDiscovery),
            warnings: Vec::new(),
        };
        if wsl_only {
            return plan;
        }

        let current = std::env::var("WSL_DISTRO_NAME").ok();
        match multiwsl::running_distros() {
            Ok(distros) => {
                plan.additional = distros
                    .into_iter()
                    .filter(|name| current.as_deref() != Some(name.as_str()))
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
            if !running.contains(name) {
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
}

#[cfg(test)]
mod tests {
    use super::{CollectorError, CollectorPlan, ProcessSnapshotCollector, RunningDistroDiscovery};
    use crate::model::Snapshot;
    use std::cell::Cell;
    use std::rc::Rc;
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

    impl RunningDistroDiscovery for StubDiscovery {
        fn running_distros(&self) -> Result<Vec<String>, CollectorError> {
            match &self.0 {
                Ok(distros) => Ok(distros.clone()),
                Err(error) => Err((*error).into()),
            }
        }
    }

    struct CountingCollector(Rc<Cell<usize>>);

    impl ProcessSnapshotCollector for CountingCollector {
        fn snapshot(&self) -> Result<Snapshot, CollectorError> {
            self.0.set(self.0.get() + 1);
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
        let calls = Rc::new(Cell::new(0));
        let plan = CollectorPlan {
            primary: Box::new(StubCollector(Ok(snapshot()))),
            additional: vec![(
                "Ubuntu-2".to_string(),
                Box::new(CountingCollector(Rc::clone(&calls))),
            )],
            distro_discovery: Box::new(StubDiscovery(Ok(Vec::new()))),
            warnings: Vec::new(),
        };

        let result = plan.capture().unwrap();
        assert_eq!(calls.get(), 0);
        assert!(result.additional.is_empty());
        assert_eq!(
            result.warnings,
            ["additional WSL Ubuntu-2 unavailable: distribution is no longer running"]
        );
    }
}
