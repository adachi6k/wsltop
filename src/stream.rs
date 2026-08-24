use crate::attribution;
use crate::docker::DockerUsage;
use crate::model::ResourceUsage;
use crate::monitor::{prepare_flat_resources, MonitorConfig, MonitorSnapshot};
use crate::wslc::WslcUsage;
use crate::{docker, linux, multiwsl, sampler, windows, wslc};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

const STARTUP_WARMUP: Duration = Duration::from_millis(150);
const SLOW_COLLECTOR_MIN_INTERVAL: Duration = Duration::from_secs(2);

enum Event {
    HostCpuCount(u32),
    Linux(Result<Vec<ResourceUsage>, String>),
    Windows(Result<(Vec<ResourceUsage>, u32), String>),
    ExtraWsl(Result<Vec<ResourceUsage>, String>),
    WslcAggregate(Result<WslcUsage, String>),
    WslcDetails(WslcUsage),
    DockerAggregate(Result<DockerUsage, String>),
    DockerDetails(DockerUsage),
}

impl Event {
    fn name(&self) -> &'static str {
        match self {
            Self::HostCpuCount(_) => "Windows host CPU",
            Self::Linux(_) => "current WSL",
            Self::Windows(_) => "Windows",
            Self::ExtraWsl(_) => "additional WSL",
            Self::WslcAggregate(_) | Self::WslcDetails(_) => "WSLC",
            Self::DockerAggregate(_) | Self::DockerDetails(_) => "Docker",
        }
    }
}

#[derive(Default)]
struct Aggregate {
    host_cpu_count: u32,
    linux: Vec<ResourceUsage>,
    windows: Vec<ResourceUsage>,
    extra_wsl: Vec<ResourceUsage>,
    wslc: WslcUsage,
    docker: DockerUsage,
    pending: BTreeSet<&'static str>,
    errors: BTreeMap<&'static str, String>,
}

impl Aggregate {
    fn new(config: &MonitorConfig) -> Self {
        let mut pending = BTreeSet::from(["current WSL"]);
        if !config.wsl_only {
            pending.extend(["Windows", "additional WSL"]);
            if !config.no_wslc {
                pending.insert("WSLC");
            }
        }
        if !config.no_docker {
            pending.insert("Docker");
        }
        Self {
            host_cpu_count: if config.wsl_only {
                fallback_cpu_count()
            } else {
                0
            },
            pending,
            ..Self::default()
        }
    }

    fn apply(&mut self, event: Event) {
        let name = event.name();
        if let Event::HostCpuCount(count) = &event {
            self.host_cpu_count = *count;
            return;
        }
        self.pending.remove(name);
        let result = match event {
            Event::HostCpuCount(_) => unreachable!(),
            Event::Linux(value) => value.map(|rows| self.linux = rows),
            Event::Windows(value) => value.map(|(rows, count)| {
                self.windows = rows;
                self.host_cpu_count = count;
            }),
            Event::ExtraWsl(value) => value.map(|rows| self.extra_wsl = rows),
            Event::WslcAggregate(value) => value.map(|mut usage| {
                usage.process_resources = self
                    .wslc
                    .process_resources
                    .iter()
                    .filter(|old| usage.resources.iter().any(|row| row.id == old.resource.id))
                    .cloned()
                    .collect();
                self.wslc = usage;
            }),
            Event::WslcDetails(usage) => {
                self.wslc = usage;
                Ok(())
            }
            Event::DockerAggregate(value) => value.map(|mut usage| {
                for item in &mut usage.resources {
                    if let Some(old) = self
                        .docker
                        .resources
                        .iter()
                        .find(|old| old.resource.id == item.resource.id)
                    {
                        item.processes.clone_from(&old.processes);
                    }
                }
                self.docker = usage;
            }),
            Event::DockerDetails(usage) => {
                self.docker = usage;
                Ok(())
            }
        };
        match result {
            Ok(()) => {
                self.errors.remove(name);
            }
            Err(error) => {
                self.errors.insert(name, error);
            }
        }
    }

    fn snapshot(&self, config: &MonitorConfig) -> MonitorSnapshot {
        let mut warnings: Vec<String> = self.errors.values().cloned().collect();
        warnings.extend(self.wslc.warnings.iter().cloned());
        warnings.extend(self.docker.warnings.iter().cloned());
        if !self.pending.is_empty() {
            warnings.push(format!(
                "loading: {}",
                self.pending.iter().copied().collect::<Vec<_>>().join(", ")
            ));
        }
        if config.wsl_only {
            warnings.push("--wsl-only uses the WSL-visible logical CPU count".to_string());
        }

        let mut linux = self.linux.clone();
        linux.extend(self.extra_wsl.clone());
        let hosts: Vec<_> = self
            .windows
            .iter()
            .filter(|row| attribution::is_host_resource(row))
            .cloned()
            .collect();
        let mut tree = attribution::build_tree_with_docker(
            self.host_cpu_count,
            &hosts,
            &linux,
            &self.wslc.resources,
            &self.docker.resources,
        );
        attribution::attach_wslc_processes(&mut tree, &self.wslc.process_resources);
        if config.hide_infra {
            attribution::hide_infra(&mut tree);
        }

        let mut resources = linux;
        resources.extend(self.windows.clone());
        resources.extend(self.wslc.resources.clone());
        if config.show_container_processes {
            append_processes(
                &mut resources,
                &self.wslc.process_resources,
                config.container_process_limit,
            );
            append_processes(
                &mut resources,
                &self.docker.resources,
                config.container_process_limit,
            );
        }
        resources.extend(
            self.docker
                .resources
                .iter()
                .map(|item| item.resource.clone()),
        );
        prepare_flat_resources(&mut resources, config);
        MonitorSnapshot {
            host_logical_cpu_count: self.host_cpu_count,
            resources,
            tree,
            warnings,
        }
    }
}

fn append_processes(
    output: &mut Vec<ResourceUsage>,
    containers: &[crate::model::ContainerProcessUsage],
    limit: usize,
) {
    for container in containers {
        let mut rows = container.processes.clone();
        rows.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent));
        rows.truncate(limit);
        output.extend(rows);
    }
}

pub fn run(
    config: Arc<Mutex<MonitorConfig>>,
    details: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    output: mpsc::Sender<Result<MonitorSnapshot, String>>,
) {
    let initial = match config.lock() {
        Ok(value) => value.clone(),
        Err(_) => return,
    };
    let host_cpu_count = Arc::new(AtomicU32::new(if initial.wsl_only {
        fallback_cpu_count()
    } else {
        0
    }));
    let (sender, receiver) = mpsc::channel();
    spawn_linux(
        sender.clone(),
        Arc::clone(&stop),
        Arc::clone(&host_cpu_count),
        initial.interval,
    );
    if !initial.wsl_only {
        spawn_windows(
            sender.clone(),
            Arc::clone(&stop),
            Arc::clone(&host_cpu_count),
            initial.interval,
        );
        spawn_extra_wsl(
            sender.clone(),
            Arc::clone(&stop),
            Arc::clone(&host_cpu_count),
            initial.interval,
        );
        if !initial.no_wslc {
            spawn_wslc(
                sender.clone(),
                Arc::clone(&stop),
                Arc::clone(&host_cpu_count),
                Arc::clone(&details),
                initial.interval,
            );
        }
    }
    if !initial.no_docker {
        spawn_docker(
            sender,
            Arc::clone(&stop),
            Arc::clone(&host_cpu_count),
            details,
            initial.interval,
        );
    }

    let mut aggregate = Aggregate::new(&initial);
    let _ = output.send(Ok(aggregate.snapshot(&initial)));
    while !stop.load(Ordering::Relaxed) {
        let Ok(event) = receiver.recv_timeout(Duration::from_millis(100)) else {
            continue;
        };
        aggregate.apply(event);
        let current = match config.lock() {
            Ok(value) => value.clone(),
            Err(_) => break,
        };
        if output.send(Ok(aggregate.snapshot(&current))).is_err() {
            break;
        }
    }
}

fn spawn_linux(
    sender: mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    cpus: Arc<AtomicU32>,
    interval: Duration,
) {
    thread::spawn(move || {
        let mut before: Option<crate::model::Snapshot> = None;
        let mut delay = Duration::ZERO;
        while wait(&stop, delay) {
            match linux::snapshot() {
                Ok(after) => {
                    let count = cpus.load(Ordering::Relaxed);
                    if let Some(old) = &before {
                        if count > 0 {
                            let rows = sampler::calculate_usage(old, &after, count);
                            if sender.send(Event::Linux(Ok(rows))).is_err() {
                                break;
                            }
                        }
                    }
                    before = Some(after);
                    delay = if delay.is_zero() {
                        STARTUP_WARMUP
                    } else {
                        interval
                    };
                }
                Err(error) => {
                    if sender.send(Event::Linux(Err(error.to_string()))).is_err() {
                        break;
                    }
                    delay = interval;
                }
            }
        }
    });
}

fn spawn_windows(
    sender: mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    cpus: Arc<AtomicU32>,
    interval: Duration,
) {
    thread::spawn(move || sample_windows(sender, stop, cpus, interval));
}

fn sample_windows(
    sender: mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    cpus: Arc<AtomicU32>,
    interval: Duration,
) {
    let mut before: Option<crate::model::WindowsSnapshot> = None;
    let mut delay = Duration::ZERO;
    while wait(&stop, delay) {
        match windows::snapshot() {
            Ok(after) => {
                let count = after.host_logical_cpu_count;
                cpus.store(count, Ordering::Relaxed);
                if sender.send(Event::HostCpuCount(count)).is_err() {
                    break;
                }
                if let Some(old) = &before {
                    let rows = sampler::calculate_usage(&old.snapshot, &after.snapshot, count);
                    if sender.send(Event::Windows(Ok((rows, count)))).is_err() {
                        break;
                    }
                }
                before = Some(after);
                delay = if before.is_some() && delay.is_zero() {
                    STARTUP_WARMUP
                } else {
                    interval
                };
            }
            Err(error) => {
                if sender
                    .send(Event::Windows(Err(format!(
                        "Windows collector unavailable: {error}"
                    ))))
                    .is_err()
                {
                    break;
                }
                delay = interval;
            }
        }
    }
}

fn spawn_extra_wsl(
    sender: mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    cpus: Arc<AtomicU32>,
    interval: Duration,
) {
    thread::spawn(move || {
        let cadence = interval.max(SLOW_COLLECTOR_MIN_INTERVAL);
        let mut before = match multiwsl::snapshots() {
            Ok(value) => value,
            Err(error) => {
                let _ = sender.send(Event::ExtraWsl(Err(format!(
                    "additional WSL unavailable: {error}"
                ))));
                Vec::new()
            }
        };
        while wait(&stop, cadence) {
            match multiwsl::snapshots() {
                Ok(after) => {
                    let mut rows = Vec::new();
                    for (name, old) in &before {
                        if let Some((_, new)) =
                            after.iter().find(|(candidate, _)| candidate == name)
                        {
                            rows.extend(sampler::calculate_usage(
                                old,
                                new,
                                cpus.load(Ordering::Relaxed),
                            ));
                        }
                    }
                    before = after;
                    if sender.send(Event::ExtraWsl(Ok(rows))).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    if sender
                        .send(Event::ExtraWsl(Err(format!(
                            "additional WSL unavailable: {error}"
                        ))))
                        .is_err()
                    {
                        break;
                    }
                }
            }
        }
    });
}

fn spawn_wslc(
    sender: mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    cpus: Arc<AtomicU32>,
    details: Arc<AtomicBool>,
    interval: Duration,
) {
    thread::spawn(move || {
        let cadence = interval.max(SLOW_COLLECTOR_MIN_INTERVAL);
        let mut last_details = WslcUsage::default();
        while let Some(count) = ready_cpu_count(&stop, &cpus) {
            match wslc::aggregate_usage(count) {
                Ok(mut usage) => {
                    if sender
                        .send(Event::WslcAggregate(Ok(usage.clone())))
                        .is_err()
                    {
                        break;
                    }
                    if details.load(Ordering::Relaxed) {
                        usage.process_resources = last_details
                            .process_resources
                            .iter()
                            .filter(|old| {
                                usage.resources.iter().any(|row| row.id == old.resource.id)
                            })
                            .cloned()
                            .collect();
                        wslc::populate_processes(&mut usage, count);
                        last_details = usage.clone();
                        if sender.send(Event::WslcDetails(usage)).is_err() {
                            break;
                        }
                    }
                }
                Err(error) => {
                    if sender
                        .send(Event::WslcAggregate(Err(format!(
                            "WSLC collector unavailable: {error}"
                        ))))
                        .is_err()
                    {
                        break;
                    }
                }
            }
            if !wait(&stop, cadence) {
                break;
            }
        }
    });
}

fn spawn_docker(
    sender: mpsc::Sender<Event>,
    stop: Arc<AtomicBool>,
    cpus: Arc<AtomicU32>,
    details: Arc<AtomicBool>,
    interval: Duration,
) {
    thread::spawn(move || {
        let cadence = interval.max(SLOW_COLLECTOR_MIN_INTERVAL);
        let mut last_details = DockerUsage::default();
        while let Some(count) = ready_cpu_count(&stop, &cpus) {
            match docker::aggregate_usage(count) {
                Ok(mut usage) => {
                    if sender
                        .send(Event::DockerAggregate(Ok(usage.clone())))
                        .is_err()
                    {
                        break;
                    }
                    if details.load(Ordering::Relaxed) {
                        for item in &mut usage.resources {
                            if let Some(old) = last_details
                                .resources
                                .iter()
                                .find(|old| old.resource.id == item.resource.id)
                            {
                                item.processes.clone_from(&old.processes);
                            }
                        }
                        docker::populate_processes(&mut usage, count);
                        last_details = usage.clone();
                        if sender.send(Event::DockerDetails(usage)).is_err() {
                            break;
                        }
                    }
                }
                Err(error) => {
                    if sender
                        .send(Event::DockerAggregate(Err(format!(
                            "Docker collector unavailable: {error}"
                        ))))
                        .is_err()
                    {
                        break;
                    }
                }
            }
            if !wait(&stop, cadence) {
                break;
            }
        }
    });
}

fn wait(stop: &AtomicBool, duration: Duration) -> bool {
    let mut remaining = duration;
    while !remaining.is_zero() && !stop.load(Ordering::Relaxed) {
        let step = remaining.min(Duration::from_millis(50));
        thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
    !stop.load(Ordering::Relaxed)
}

fn ready_cpu_count(stop: &AtomicBool, cpus: &AtomicU32) -> Option<u32> {
    loop {
        let count = cpus.load(Ordering::Relaxed);
        if count > 0 {
            return Some(count);
        }
        if !wait(stop, Duration::from_millis(50)) {
            return None;
        }
    }
}

fn fallback_cpu_count() -> u32 {
    thread::available_parallelism().map_or(1, |count| count.get() as u32)
}

#[cfg(test)]
mod tests {
    use super::{Aggregate, Event};
    use crate::docker::DockerUsage;
    use crate::model::{ContainerProcessUsage, EnvironmentKind, ResourceKind, ResourceUsage};
    use crate::monitor::MonitorConfig;
    use std::time::Duration;

    fn config() -> MonitorConfig {
        MonitorConfig {
            interval: Duration::from_secs(1),
            limit: 30,
            show_wsl_host: false,
            wsl_only: false,
            no_wslc: false,
            no_docker: false,
            hide_infra: false,
            show_container_processes: false,
            container_process_limit: 5,
        }
    }

    fn row(environment: EnvironmentKind, kind: ResourceKind, id: &str) -> ResourceUsage {
        ResourceUsage {
            environment,
            source: None,
            kind,
            id: id.into(),
            pid: None,
            ppid: None,
            name: id.into(),
            args: None,
            cpu_percent: 1.0,
            memory_bytes: 1,
        }
    }

    #[test]
    fn partial_success_clears_loading_without_discarding_other_data() {
        let mut aggregate = Aggregate::new(&config());
        let row = ResourceUsage {
            environment: EnvironmentKind::Wsl,
            source: None,
            kind: ResourceKind::Process,
            id: "1".into(),
            pid: Some(1),
            ppid: None,
            name: "work".into(),
            args: None,
            cpu_percent: 2.0,
            memory_bytes: 1,
        };
        aggregate.apply(Event::Linux(Ok(vec![row])));
        let snapshot = aggregate.snapshot(&config());
        assert_eq!(snapshot.resources.len(), 1);
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("loading:")));
        assert!(!snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("current WSL")));
    }

    #[test]
    fn collector_error_preserves_last_good_data() {
        let mut aggregate = Aggregate::new(&config());
        aggregate.apply(Event::Linux(Ok(Vec::new())));
        aggregate.apply(Event::Linux(Err("failed".into())));
        assert!(aggregate
            .snapshot(&config())
            .warnings
            .iter()
            .any(|warning| warning == "failed"));
    }

    #[test]
    fn docker_aggregate_refresh_preserves_last_process_details() {
        let mut aggregate = Aggregate::new(&config());
        let container = row(
            EnvironmentKind::Docker,
            ResourceKind::Container,
            "container",
        );
        let process = row(EnvironmentKind::Docker, ResourceKind::Process, "process");
        aggregate.apply(Event::DockerDetails(DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: container.clone(),
                processes: vec![process],
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        }));
        aggregate.apply(Event::DockerAggregate(Ok(DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: container,
                processes: Vec::new(),
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        })));
        assert_eq!(aggregate.docker.resources[0].processes.len(), 1);
    }
}
