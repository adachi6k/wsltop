use crate::attribution;
use crate::docker::DockerUsage;
use crate::model::{ContainerProcessUsage, ResourceUsage};
use crate::monitor::{prepare_flat_resources, MonitorConfig, MonitorSnapshot};
use crate::windows_app::WindowsMetadata;
use crate::wslc::WslcUsage;
use crate::{docker, linux, multiwsl, sampler, windows, windows_app, wslc};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

const STARTUP_WARMUP: Duration = Duration::from_millis(150);
const SLOW_COLLECTOR_MIN_INTERVAL: Duration = Duration::from_secs(2);

enum Event {
    HostCpuCount(u32),
    Linux(Normalized<Result<Vec<ResourceUsage>, String>>),
    Windows(Result<(Vec<ResourceUsage>, u32), String>),
    WindowsMetadata(Result<WindowsMetadata, String>),
    ExtraWsl(Normalized<Result<ExtraWslUpdate, String>>),
    WslcAggregate(Normalized<Result<WslcUsage, String>>),
    WslcDetails(Normalized<WslcUsage>),
    DockerAggregate(Normalized<Result<DockerUsage, String>>),
    DockerDetails(Normalized<DockerUsage>),
}

struct Normalized<T> {
    cpu_count: u32,
    value: T,
}

impl<T> Normalized<T> {
    fn new(cpu_count: u32, value: T) -> Self {
        Self { cpu_count, value }
    }
}

#[derive(Default)]
struct ExtraWslUpdate {
    rows: Vec<ResourceUsage>,
    running_sources: BTreeSet<String>,
    successful_sources: BTreeSet<String>,
    failures: BTreeMap<String, String>,
}

impl Event {
    fn name(&self) -> &'static str {
        match self {
            Self::HostCpuCount(_) => "Windows host CPU",
            Self::Linux(_) => "current WSL",
            Self::Windows(_) => "Windows",
            Self::WindowsMetadata(_) => "Windows applications",
            Self::ExtraWsl(_) => "additional WSL",
            Self::WslcAggregate(_) | Self::WslcDetails(_) => "WSLC",
            Self::DockerAggregate(_) | Self::DockerDetails(_) => "Docker",
        }
    }
}

#[derive(Default)]
struct Aggregate {
    host_cpu_count: u32,
    host_cpu_authoritative: bool,
    linux: Vec<ResourceUsage>,
    windows: Vec<ResourceUsage>,
    windows_metadata: WindowsMetadata,
    extra_wsl: BTreeMap<String, Vec<ResourceUsage>>,
    extra_wsl_errors: BTreeMap<String, String>,
    wslc: WslcUsage,
    docker: DockerUsage,
    wslc_detail_warnings: Vec<String>,
    docker_detail_warnings: Vec<String>,
    normalized_collectors: BTreeSet<&'static str>,
    pending: BTreeSet<&'static str>,
    errors: BTreeMap<&'static str, String>,
}

impl Aggregate {
    fn new(config: &MonitorConfig) -> Self {
        let mut pending = BTreeSet::from(["current WSL"]);
        let mut normalized_collectors = BTreeSet::from(["current WSL"]);
        if !config.wsl_only {
            pending.extend(["Windows", "Windows applications", "additional WSL"]);
            normalized_collectors.insert("additional WSL");
            if !config.no_wslc {
                pending.insert("WSLC");
                normalized_collectors.insert("WSLC");
            }
        }
        if !config.no_docker {
            pending.insert("Docker");
            normalized_collectors.insert("Docker");
        }
        Self {
            host_cpu_count: fallback_cpu_count(),
            host_cpu_authoritative: config.wsl_only,
            normalized_collectors,
            pending,
            ..Self::default()
        }
    }

    fn apply(&mut self, event: Event) {
        let name = event.name();
        if let Event::HostCpuCount(count) = &event {
            if self.host_cpu_count != *count {
                self.linux.clear();
                self.extra_wsl.clear();
                self.wslc = WslcUsage::default();
                self.docker = DockerUsage::default();
                self.wslc_detail_warnings.clear();
                self.docker_detail_warnings.clear();
                self.pending
                    .extend(self.normalized_collectors.iter().copied());
            }
            self.host_cpu_count = *count;
            self.host_cpu_authoritative = true;
            return;
        }
        if event.has_stale_normalization(self.host_cpu_authoritative, self.host_cpu_count) {
            return;
        }
        if let Event::WslcDetails(normalized) = &event {
            let usage = &normalized.value;
            for current in &self.wslc.resources {
                let Some(detail) = usage
                    .process_resources
                    .iter()
                    .find(|item| item.resource.id == current.id)
                else {
                    continue;
                };
                if let Some(existing) = self
                    .wslc
                    .process_resources
                    .iter_mut()
                    .find(|item| item.resource.id == current.id)
                {
                    existing.resource.clone_from(current);
                    existing.processes.clone_from(&detail.processes);
                    existing.host_pids.clone_from(&detail.host_pids);
                } else {
                    self.wslc.process_resources.push(ContainerProcessUsage {
                        resource: current.clone(),
                        processes: detail.processes.clone(),
                        host_pids: detail.host_pids.clone(),
                    });
                }
            }
            self.wslc_detail_warnings.clone_from(&usage.warnings);
            return;
        }
        if let Event::DockerDetails(normalized) = &event {
            let usage = &normalized.value;
            for current in &mut self.docker.resources {
                if let Some(detail) = usage
                    .resources
                    .iter()
                    .find(|item| item.resource.id == current.resource.id)
                {
                    current.processes.clone_from(&detail.processes);
                }
            }
            self.docker_detail_warnings.clone_from(&usage.warnings);
            return;
        }
        self.pending.remove(name);
        let result = match event {
            Event::HostCpuCount(_) => unreachable!(),
            Event::Linux(value) => value.value.map(|rows| self.linux = rows),
            Event::Windows(value) => value.map(|(rows, count)| {
                self.windows = rows;
                self.host_cpu_count = count;
            }),
            Event::WindowsMetadata(value) => value.map(|metadata| {
                self.windows_metadata = metadata;
            }),
            Event::ExtraWsl(value) => value.value.map(|update| {
                self.extra_wsl
                    .retain(|source, _| update.running_sources.contains(source));
                for source in update.successful_sources {
                    let rows = update
                        .rows
                        .iter()
                        .filter(|row| row.source.as_deref() == Some(source.as_str()))
                        .cloned()
                        .collect();
                    self.extra_wsl.insert(source, rows);
                }
                self.extra_wsl_errors = update.failures;
            }),
            Event::WslcAggregate(value) => value.value.map(|mut usage| {
                usage.process_resources = self
                    .wslc
                    .process_resources
                    .iter()
                    .filter_map(|old| {
                        usage
                            .resources
                            .iter()
                            .find(|current| current.id == old.resource.id)
                            .map(|current| ContainerProcessUsage {
                                resource: current.clone(),
                                processes: old.processes.clone(),
                                host_pids: old.host_pids.clone(),
                            })
                    })
                    .collect();
                self.wslc = usage;
            }),
            Event::WslcDetails(_) => unreachable!(),
            Event::DockerAggregate(value) => value.value.map(|mut usage| {
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
            Event::DockerDetails(_) => unreachable!(),
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
        warnings.extend(
            self.extra_wsl_errors
                .iter()
                .map(|(source, error)| format!("additional WSL {source} unavailable: {error}")),
        );
        warnings.extend(self.wslc.warnings.iter().cloned());
        warnings.extend(self.wslc_detail_warnings.iter().cloned());
        warnings.extend(self.docker.warnings.iter().cloned());
        warnings.extend(self.docker_detail_warnings.iter().cloned());
        if !self.pending.is_empty() {
            warnings.push(format!(
                "loading: {}",
                self.pending.iter().copied().collect::<Vec<_>>().join(", ")
            ));
        }
        if !config.wsl_only && !self.host_cpu_authoritative {
            warnings.push(
                "non-Windows CPU is provisional until the Windows host CPU count is available"
                    .to_string(),
            );
        }
        if config.wsl_only {
            warnings.push("--wsl-only uses the WSL-visible logical CPU count".to_string());
        }

        let mut linux = self.linux.clone();
        linux.extend(self.extra_wsl.values().flatten().cloned());
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
        let applications = windows_app::group_processes(&self.windows, &self.windows_metadata);
        tree.windows_applications.clone_from(&applications);
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
        let mut pid_resources = resources.clone();
        prepare_flat_resources(&mut pid_resources, config);
        resources.retain(|row| {
            row.environment != crate::model::EnvironmentKind::Windows
                || row.kind != crate::model::ResourceKind::Process
        });
        resources.extend(
            applications
                .into_iter()
                .map(|application| application.resource),
        );
        prepare_flat_resources(&mut resources, config);
        MonitorSnapshot {
            host_logical_cpu_count: self.host_cpu_count,
            resources,
            pid_resources,
            tree,
            warnings,
        }
    }
}

impl Event {
    fn has_stale_normalization(&self, authoritative: bool, current_count: u32) -> bool {
        if !authoritative {
            return false;
        }
        match self {
            Self::Linux(value) => value.cpu_count != current_count,
            Self::ExtraWsl(value) => value.cpu_count != current_count,
            Self::WslcAggregate(value) => value.cpu_count != current_count,
            Self::WslcDetails(value) => value.cpu_count != current_count,
            Self::DockerAggregate(value) => value.cpu_count != current_count,
            Self::DockerDetails(value) => value.cpu_count != current_count,
            Self::HostCpuCount(_) | Self::Windows(_) | Self::WindowsMetadata(_) => false,
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

fn reuse_wslc_processes(
    usage: &mut WslcUsage,
    cached: Option<&Normalized<WslcUsage>>,
    cpu_count: u32,
) {
    usage.process_resources = cached
        .filter(|old| old.cpu_count == cpu_count)
        .map(|old| {
            old.value
                .process_resources
                .iter()
                .filter(|old| usage.resources.iter().any(|row| row.id == old.resource.id))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
}

fn reuse_docker_processes(
    usage: &mut DockerUsage,
    cached: Option<&Normalized<DockerUsage>>,
    cpu_count: u32,
) {
    let Some(old) = cached.filter(|old| old.cpu_count == cpu_count) else {
        for item in &mut usage.resources {
            item.processes.clear();
        }
        return;
    };
    for item in &mut usage.resources {
        if let Some(old) = old
            .value
            .resources
            .iter()
            .find(|old| old.resource.id == item.resource.id)
        {
            item.processes.clone_from(&old.processes);
        }
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
    let host_cpu_count = Arc::new(AtomicU32::new(fallback_cpu_count()));
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
        spawn_windows_metadata(sender.clone(), Arc::clone(&stop));
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
                    let had_baseline = before.is_some();
                    let count = match cpus.load(Ordering::Relaxed) {
                        0 => fallback_cpu_count(),
                        count => count,
                    };
                    if let Some(old) = &before {
                        let rows = sampler::calculate_usage(old, &after, count);
                        if sender
                            .send(Event::Linux(Normalized::new(count, Ok(rows))))
                            .is_err()
                        {
                            break;
                        }
                    }
                    before = Some(after);
                    delay = if had_baseline {
                        interval
                    } else {
                        STARTUP_WARMUP
                    };
                }
                Err(error) => {
                    let count = cpus.load(Ordering::Relaxed);
                    if sender
                        .send(Event::Linux(Normalized::new(count, Err(error.to_string()))))
                        .is_err()
                    {
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

fn spawn_windows_metadata(sender: mpsc::Sender<Event>, stop: Arc<AtomicBool>) {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let event = match windows::application_metadata() {
                Ok(metadata) => Event::WindowsMetadata(Ok(metadata)),
                Err(error) => Event::WindowsMetadata(Err(format!(
                    "Windows application metadata unavailable: {error}"
                ))),
            };
            if sender.send(event).is_err() {
                break;
            }
            if !wait(&stop, Duration::from_secs(10)) {
                break;
            }
        }
    });
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
                delay = interval;
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
            Ok(value) => value.snapshots,
            Err(error) => {
                let _ = sender.send(Event::ExtraWsl(Normalized::new(
                    cpus.load(Ordering::Relaxed),
                    Err(format!("additional WSL unavailable: {error}")),
                )));
                Vec::new()
            }
        };
        while wait(&stop, cadence) {
            match multiwsl::snapshots() {
                Ok(after) => {
                    let count = cpus.load(Ordering::Relaxed);
                    let mut rows = Vec::new();
                    let running_sources: BTreeSet<String> = after
                        .snapshots
                        .iter()
                        .map(|(name, _)| name.clone())
                        .chain(after.failures.iter().map(|(name, _)| name.clone()))
                        .collect();
                    let successful_sources: BTreeSet<String> = after
                        .snapshots
                        .iter()
                        .map(|(name, _)| name.clone())
                        .collect();
                    for (name, old) in &before {
                        if let Some((_, new)) = after
                            .snapshots
                            .iter()
                            .find(|(candidate, _)| candidate == name)
                        {
                            rows.extend(sampler::calculate_usage(old, new, count));
                        }
                    }
                    before.retain(|(name, _)| running_sources.contains(name));
                    for (name, snapshot) in after.snapshots {
                        if let Some((_, old)) = before.iter_mut().find(|(old, _)| old == &name) {
                            *old = snapshot;
                        } else {
                            before.push((name, snapshot));
                        }
                    }
                    if sender
                        .send(Event::ExtraWsl(Normalized::new(
                            count,
                            Ok(ExtraWslUpdate {
                                rows,
                                running_sources,
                                successful_sources,
                                failures: after.failures.into_iter().collect(),
                            }),
                        )))
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    if sender
                        .send(Event::ExtraWsl(Normalized::new(
                            cpus.load(Ordering::Relaxed),
                            Err(format!("additional WSL unavailable: {error}")),
                        )))
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
    let (detail_sender, detail_receiver) = mpsc::sync_channel::<(WslcUsage, u32)>(1);
    let detail_events = sender.clone();
    let detail_stop = Arc::clone(&stop);
    thread::spawn(move || {
        let mut last_details: Option<Normalized<WslcUsage>> = None;
        while !detail_stop.load(Ordering::Relaxed) {
            let Ok((mut usage, count)) = detail_receiver.recv_timeout(Duration::from_millis(100))
            else {
                continue;
            };
            reuse_wslc_processes(&mut usage, last_details.as_ref(), count);
            usage.warnings.clear();
            wslc::populate_processes(&mut usage, count);
            last_details = Some(Normalized::new(count, usage.clone()));
            if detail_events
                .send(Event::WslcDetails(Normalized::new(count, usage)))
                .is_err()
            {
                break;
            }
        }
    });
    thread::spawn(move || {
        let cadence = interval.max(SLOW_COLLECTOR_MIN_INTERVAL);
        while let Some(count) = ready_cpu_count(&stop, &cpus) {
            match wslc::aggregate_usage(count) {
                Ok(usage) => {
                    if sender
                        .send(Event::WslcAggregate(Normalized::new(
                            count,
                            Ok(usage.clone()),
                        )))
                        .is_err()
                    {
                        break;
                    }
                    if details.load(Ordering::Relaxed) {
                        let _ = detail_sender.try_send((usage, count));
                    }
                }
                Err(error) => {
                    if sender
                        .send(Event::WslcAggregate(Normalized::new(
                            count,
                            Err(format!("WSLC collector unavailable: {error}")),
                        )))
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
    let (detail_sender, detail_receiver) = mpsc::sync_channel::<(DockerUsage, u32)>(1);
    let detail_events = sender.clone();
    let detail_stop = Arc::clone(&stop);
    thread::spawn(move || {
        let mut last_details: Option<Normalized<DockerUsage>> = None;
        while !detail_stop.load(Ordering::Relaxed) {
            let Ok((mut usage, count)) = detail_receiver.recv_timeout(Duration::from_millis(100))
            else {
                continue;
            };
            reuse_docker_processes(&mut usage, last_details.as_ref(), count);
            usage.warnings.clear();
            docker::populate_processes(&mut usage, count);
            last_details = Some(Normalized::new(count, usage.clone()));
            if detail_events
                .send(Event::DockerDetails(Normalized::new(count, usage)))
                .is_err()
            {
                break;
            }
        }
    });
    thread::spawn(move || {
        let cadence = interval.max(SLOW_COLLECTOR_MIN_INTERVAL);
        while let Some(count) = ready_cpu_count(&stop, &cpus) {
            match docker::aggregate_usage(count) {
                Ok(usage) => {
                    if sender
                        .send(Event::DockerAggregate(Normalized::new(
                            count,
                            Ok(usage.clone()),
                        )))
                        .is_err()
                    {
                        break;
                    }
                    if details.load(Ordering::Relaxed) {
                        let _ = detail_sender.try_send((usage, count));
                    }
                }
                Err(error) => {
                    if sender
                        .send(Event::DockerAggregate(Normalized::new(
                            count,
                            Err(format!("Docker collector unavailable: {error}")),
                        )))
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
    use super::{
        fallback_cpu_count, reuse_docker_processes, reuse_wslc_processes, Aggregate, Event,
        ExtraWslUpdate, Normalized,
    };
    use crate::docker::DockerUsage;
    use crate::model::{ContainerProcessUsage, EnvironmentKind, ResourceKind, ResourceUsage};
    use crate::monitor::MonitorConfig;
    use crate::windows_app::{WindowsMetadata, WindowsProcessMetadata};
    use crate::wslc::WslcUsage;
    use std::collections::{BTreeMap, BTreeSet};
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

    fn normalized<T>(value: T) -> Normalized<T> {
        Normalized::new(fallback_cpu_count(), value)
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
        aggregate.apply(Event::Linux(normalized(Ok(vec![row]))));
        let snapshot = aggregate.snapshot(&config());
        assert_eq!(snapshot.resources.len(), 1);
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("loading:")));
        let loading = snapshot
            .warnings
            .iter()
            .find(|warning| warning.starts_with("loading:"))
            .unwrap();
        assert!(!loading.contains("current WSL"));
    }

    #[test]
    fn collector_error_preserves_last_good_data() {
        let mut aggregate = Aggregate::new(&config());
        aggregate.apply(Event::Linux(normalized(Ok(Vec::new()))));
        aggregate.apply(Event::Linux(normalized(Err("failed".into()))));
        assert!(aggregate
            .snapshot(&config())
            .warnings
            .iter()
            .any(|warning| warning == "failed"));
    }

    #[test]
    fn additional_wsl_failure_preserves_only_that_distros_last_good_rows() {
        let mut aggregate = Aggregate::new(&config());
        let mut process = row(EnvironmentKind::Wsl, ResourceKind::Process, "42");
        process.source = Some("Ubuntu-2".into());
        aggregate.apply(Event::ExtraWsl(normalized(Ok(ExtraWslUpdate {
            rows: vec![process],
            running_sources: BTreeSet::from(["Ubuntu-2".into()]),
            successful_sources: BTreeSet::from(["Ubuntu-2".into()]),
            failures: BTreeMap::new(),
        }))));
        aggregate.apply(Event::ExtraWsl(normalized(Ok(ExtraWslUpdate {
            running_sources: BTreeSet::from(["Ubuntu-2".into()]),
            failures: BTreeMap::from([("Ubuntu-2".into(), "remote /proc failed".into())]),
            ..ExtraWslUpdate::default()
        }))));

        let snapshot = aggregate.snapshot(&config());
        assert!(snapshot
            .resources
            .iter()
            .any(|row| { row.source.as_deref() == Some("Ubuntu-2") && row.id == "42" }));
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("Ubuntu-2") && warning.contains("remote /proc")));
    }

    #[test]
    fn host_count_event_replaces_provisional_normalization_status() {
        let mut aggregate = Aggregate::new(&config());
        assert!(aggregate
            .snapshot(&config())
            .warnings
            .iter()
            .any(|warning| warning.contains("provisional")));
        aggregate.apply(Event::HostCpuCount(32));
        let snapshot = aggregate.snapshot(&config());
        assert_eq!(snapshot.host_logical_cpu_count, 32);
        assert!(!snapshot
            .warnings
            .iter()
            .any(|warning| warning.contains("provisional")));
    }

    #[test]
    fn authoritative_host_count_invalidates_and_rejects_old_scale_rows() {
        let mut aggregate = Aggregate::new(&config());
        let fallback = aggregate.host_cpu_count;
        let authoritative = fallback.saturating_add(1);
        let process = row(EnvironmentKind::Wsl, ResourceKind::Process, "old-scale");
        aggregate.apply(Event::Linux(Normalized::new(
            fallback,
            Ok(vec![process.clone()]),
        )));
        assert_eq!(aggregate.linux.len(), 1);

        aggregate.apply(Event::HostCpuCount(authoritative));
        assert!(aggregate.linux.is_empty());
        assert!(aggregate.pending.contains("current WSL"));

        aggregate.apply(Event::Linux(Normalized::new(
            fallback,
            Ok(vec![process.clone()]),
        )));
        assert!(aggregate.linux.is_empty());
        assert!(aggregate.pending.contains("current WSL"));

        aggregate.apply(Event::Linux(Normalized::new(
            authoritative,
            Ok(vec![process]),
        )));
        assert_eq!(aggregate.linux.len(), 1);
        assert!(!aggregate.pending.contains("current WSL"));
    }

    #[test]
    fn unchanged_authoritative_host_count_keeps_correctly_scaled_rows() {
        let mut aggregate = Aggregate::new(&config());
        let count = aggregate.host_cpu_count;
        aggregate.apply(Event::Linux(Normalized::new(
            count,
            Ok(vec![row(
                EnvironmentKind::Wsl,
                ResourceKind::Process,
                "same-scale",
            )]),
        )));
        aggregate.apply(Event::HostCpuCount(count));
        assert_eq!(aggregate.linux.len(), 1);
    }

    #[test]
    fn windows_applications_are_ranked_once_while_pid_rows_remain_for_json() {
        let mut options = config();
        options.limit = 1;
        let mut aggregate = Aggregate::new(&options);
        let mut first = row(EnvironmentKind::Windows, ResourceKind::Process, "10");
        first.pid = Some(10);
        first.name = "chrome".into();
        first.cpu_percent = 3.0;
        let mut second = row(EnvironmentKind::Windows, ResourceKind::Process, "11");
        second.pid = Some(11);
        second.name = "chrome".into();
        second.cpu_percent = 2.0;
        aggregate.apply(Event::Windows(Ok((vec![first, second], 16))));
        aggregate.apply(Event::WindowsMetadata(Ok(WindowsMetadata::new())));

        let snapshot = aggregate.snapshot(&options);
        assert_eq!(snapshot.resources.len(), 1);
        assert_eq!(snapshot.resources[0].kind, ResourceKind::Application);
        assert_eq!(snapshot.resources[0].name, "Chrome");
        assert_eq!(snapshot.resources[0].cpu_percent, 5.0);
        assert!(snapshot
            .pid_resources
            .iter()
            .all(|row| row.kind == ResourceKind::Process));
        assert_eq!(snapshot.tree.windows_applications[0].processes.len(), 2);
    }

    #[test]
    fn windows_metadata_error_retains_last_good_application_ownership() {
        let mut aggregate = Aggregate::new(&config());
        let mut teams = row(EnvironmentKind::Windows, ResourceKind::Process, "10");
        teams.pid = Some(10);
        teams.name = "ms-teams".into();
        let mut webview = row(EnvironmentKind::Windows, ResourceKind::Process, "11");
        webview.pid = Some(11);
        webview.name = "msedgewebview2".into();
        aggregate.apply(Event::Windows(Ok((vec![teams, webview], 16))));
        aggregate.apply(Event::WindowsMetadata(Ok(WindowsMetadata::from([(
            11,
            WindowsProcessMetadata {
                pid: 11,
                parent_pid: 10,
                name: "msedgewebview2.exe".into(),
                executable_path: None,
                command_line: None,
            },
        )]))));
        aggregate.apply(Event::WindowsMetadata(Err("metadata failed".into())));

        let snapshot = aggregate.snapshot(&config());
        let teams = snapshot
            .tree
            .windows_applications
            .iter()
            .find(|application| application.resource.name == "Teams")
            .unwrap();
        assert_eq!(teams.processes.len(), 2);
        assert!(snapshot
            .warnings
            .iter()
            .any(|warning| warning == "metadata failed"));
    }

    #[test]
    fn detail_workers_reuse_cache_only_on_the_same_cpu_scale() {
        let container = row(
            EnvironmentKind::WslContainer,
            ResourceKind::Container,
            "container",
        );
        let process = row(
            EnvironmentKind::WslContainer,
            ResourceKind::Process,
            "process",
        );
        let cached_wslc = Normalized::new(
            8,
            WslcUsage {
                resources: vec![container.clone()],
                process_resources: vec![ContainerProcessUsage {
                    resource: container.clone(),
                    processes: vec![process.clone()],
                    host_pids: Vec::new(),
                }],
                warnings: Vec::new(),
            },
        );
        let mut fresh_wslc = WslcUsage {
            resources: vec![container],
            ..WslcUsage::default()
        };
        reuse_wslc_processes(&mut fresh_wslc, Some(&cached_wslc), 16);
        assert!(fresh_wslc.process_resources.is_empty());

        let docker_container = row(EnvironmentKind::Docker, ResourceKind::Container, "docker");
        let cached_docker = Normalized::new(
            8,
            DockerUsage {
                resources: vec![ContainerProcessUsage {
                    resource: docker_container.clone(),
                    processes: vec![process],
                    host_pids: Vec::new(),
                }],
                warnings: Vec::new(),
            },
        );
        let mut fresh_docker = DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: docker_container,
                processes: Vec::new(),
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        };
        reuse_docker_processes(&mut fresh_docker, Some(&cached_docker), 16);
        assert!(fresh_docker.resources[0].processes.is_empty());
    }

    #[test]
    fn aggregate_refresh_does_not_clear_detail_status() {
        let mut aggregate = Aggregate::new(&config());
        let container = row(
            EnvironmentKind::Docker,
            ResourceKind::Container,
            "container",
        );
        let usage = || DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: container.clone(),
                processes: Vec::new(),
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        };
        aggregate.apply(Event::DockerAggregate(normalized(Ok(usage()))));
        let mut detail = usage();
        detail.warnings.push("Docker detail unavailable".into());
        aggregate.apply(Event::DockerDetails(normalized(detail)));
        aggregate.apply(Event::DockerAggregate(normalized(Ok(usage()))));
        assert!(aggregate
            .snapshot(&config())
            .warnings
            .iter()
            .any(|warning| warning == "Docker detail unavailable"));

        aggregate.apply(Event::DockerDetails(normalized(usage())));
        assert!(!aggregate
            .snapshot(&config())
            .warnings
            .iter()
            .any(|warning| warning == "Docker detail unavailable"));
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
        aggregate.apply(Event::DockerAggregate(normalized(Ok(DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: container.clone(),
                processes: Vec::new(),
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        }))));
        aggregate.apply(Event::DockerDetails(normalized(DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: container.clone(),
                processes: vec![process],
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        })));
        aggregate.apply(Event::DockerAggregate(normalized(Ok(DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: container,
                processes: Vec::new(),
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        }))));
        assert_eq!(aggregate.docker.resources[0].processes.len(), 1);
    }

    #[test]
    fn stale_docker_details_do_not_roll_back_aggregate_or_clear_error() {
        let mut aggregate = Aggregate::new(&config());
        let mut current = row(
            EnvironmentKind::Docker,
            ResourceKind::Container,
            "container",
        );
        current.cpu_percent = 9.0;
        aggregate.apply(Event::DockerAggregate(normalized(Ok(DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: current,
                processes: Vec::new(),
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        }))));
        aggregate.apply(Event::DockerAggregate(normalized(Err(
            "new aggregate error".into(),
        ))));

        let stale = row(
            EnvironmentKind::Docker,
            ResourceKind::Container,
            "container",
        );
        let detail = row(EnvironmentKind::Docker, ResourceKind::Process, "process");
        aggregate.apply(Event::DockerDetails(normalized(DockerUsage {
            resources: vec![ContainerProcessUsage {
                resource: stale,
                processes: vec![detail],
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        })));

        assert_eq!(aggregate.docker.resources[0].resource.cpu_percent, 9.0);
        assert_eq!(aggregate.docker.resources[0].processes.len(), 1);
        assert_eq!(
            aggregate.errors.get("Docker").unwrap(),
            "new aggregate error"
        );
    }

    #[test]
    fn wslc_details_always_use_the_latest_aggregate_resource() {
        let mut aggregate = Aggregate::new(&config());
        let mut current = row(
            EnvironmentKind::WslContainer,
            ResourceKind::Container,
            "container",
        );
        current.cpu_percent = 9.0;
        aggregate.apply(Event::WslcAggregate(normalized(Ok(WslcUsage {
            resources: vec![current],
            process_resources: Vec::new(),
            warnings: Vec::new(),
        }))));

        let stale = row(
            EnvironmentKind::WslContainer,
            ResourceKind::Container,
            "container",
        );
        let process = row(
            EnvironmentKind::WslContainer,
            ResourceKind::Process,
            "process",
        );
        aggregate.apply(Event::WslcDetails(normalized(WslcUsage {
            resources: vec![stale.clone()],
            process_resources: vec![ContainerProcessUsage {
                resource: stale,
                processes: vec![process],
                host_pids: Vec::new(),
            }],
            warnings: Vec::new(),
        })));
        assert_eq!(
            aggregate.wslc.process_resources[0].resource.cpu_percent,
            9.0
        );

        let mut newer = row(
            EnvironmentKind::WslContainer,
            ResourceKind::Container,
            "container",
        );
        newer.cpu_percent = 12.0;
        aggregate.apply(Event::WslcAggregate(normalized(Ok(WslcUsage {
            resources: vec![newer],
            process_resources: Vec::new(),
            warnings: Vec::new(),
        }))));
        assert_eq!(
            aggregate.wslc.process_resources[0].resource.cpu_percent,
            12.0
        );
        assert_eq!(aggregate.wslc.process_resources[0].processes.len(), 1);
    }
}
