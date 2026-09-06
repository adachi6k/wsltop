use crate::attribution::{self, AttributionTree};
use crate::collector::CollectorPlan;
use crate::model::{ResourceKind, ResourceUsage, WindowsApplicationUsage};
use crate::query::{QuerySource, ResourceQuery, Sort};
use crate::{docker, sampler, windows, windows_app, wslc};
use std::error::Error;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub sort: Sort,
    pub interval: Duration,
    pub limit: usize,
    pub show_wsl_host: bool,
    pub wsl_only: bool,
    pub no_wslc: bool,
    pub no_docker: bool,
    pub hide_infra: bool,
    pub show_container_processes: bool,
    pub container_process_limit: usize,
    pub collect_windows_applications: bool,
}

pub struct Monitor {
    config: MonitorConfig,
    distro: Option<String>,
}

pub struct MonitorSnapshot {
    pub host_cpu_percent: Option<f64>,
    pub sort: Sort,
    pub query_source: Option<QuerySource>,
    pub host_logical_cpu_count: u32,
    pub resources: Vec<ResourceUsage>,
    pub pid_resources: Vec<ResourceUsage>,
    pub tree: AttributionTree,
    pub warnings: Vec<String>,
}

impl MonitorConfig {
    pub fn query(&self) -> ResourceQuery {
        ResourceQuery {
            sort: self.sort,
            limit: self.limit,
            container_process_limit: self.container_process_limit,
            show_container_processes: self.show_container_processes,
            show_wsl_host: self.show_wsl_host,
            hide_infra: self.hide_infra,
        }
    }
}

impl MonitorSnapshot {
    pub fn requery(&mut self, query: &ResourceQuery) {
        if let Some(source) = &self.query_source {
            self.sort = query.sort;
            self.resources = query.flat(&source.resources);
            self.pid_resources = query.flat(&source.pid_resources);
            self.tree = query.tree(&source.tree);
        }
    }

    pub(crate) fn from_collected(
        pid_resources: Vec<ResourceUsage>,
        tree: AttributionTree,
        warnings: Vec<String>,
        config: &MonitorConfig,
    ) -> Self {
        let mut resources = pid_resources.clone();
        apply_windows_application_view(&mut resources, &tree.windows_applications, config);
        let query = config.query();
        Self {
            host_cpu_percent: None,
            sort: config.sort,
            host_logical_cpu_count: tree.host_logical_cpu_count,
            resources: query.flat(&resources),
            pid_resources: query.flat(&pid_resources),
            tree: query.tree(&tree),
            warnings,
            query_source: Some(QuerySource {
                resources,
                pid_resources,
                tree,
            }),
        }
    }
}

impl Monitor {
    pub fn new(config: MonitorConfig, distro: Option<String>) -> Self {
        Self { config, distro }
    }
    pub fn sample(&mut self) -> Result<MonitorSnapshot, Box<dyn Error>> {
        let mut warnings = Vec::new();
        let collector_plan = CollectorPlan::native(self.distro.as_deref(), self.config.wsl_only)?;
        let linux_before = collector_plan.capture()?;
        for warning in linux_before.warnings {
            push_unique_warning(&mut warnings, warning);
        }
        let windows_before = if self.config.wsl_only {
            None
        } else {
            Some(windows::snapshot()?)
        };
        let collector_cpu_count = match windows_before.as_ref() {
            Some(snapshot) => snapshot.host_logical_cpu_count,
            None => wsl_only_host_cpu_count()?,
        };
        let collect_wslc = !self.config.wsl_only && !self.config.no_wslc;
        let collect_docker = !self.config.no_docker;
        let wslc_worker = collect_wslc.then(|| {
            thread::spawn(move || {
                wslc::usage(collector_cpu_count).map_err(|error| error.to_string())
            })
        });
        let docker_worker = collect_docker.then(|| {
            thread::spawn(move || {
                docker::usage(collector_cpu_count).map_err(|error| error.to_string())
            })
        });
        thread::sleep(self.config.interval);
        let after_result = (|| -> Result<_, Box<dyn Error>> {
            let linux_after = collector_plan.capture()?;
            let windows_after = if self.config.wsl_only {
                None
            } else {
                Some(windows::snapshot()?)
            };
            Ok((linux_after, windows_after))
        })();
        let mut wslc_result = wslc_worker.map(|worker| {
            worker
                .join()
                .unwrap_or_else(|_| Err("WSLC collector panicked".to_string()))
        });
        let mut docker_result = docker_worker.map(|worker| {
            worker
                .join()
                .unwrap_or_else(|_| Err("Docker collector panicked".to_string()))
        });
        let (linux_after, windows_after) = after_result?;
        for warning in linux_after.warnings {
            push_unique_warning(&mut warnings, warning);
        }
        let host_cpu_count = windows_after
            .as_ref()
            .map_or(collector_cpu_count, |snapshot| {
                snapshot.host_logical_cpu_count
            });
        if self.config.wsl_only {
            warnings.push(wsl_only_warning().to_string());
        }

        let mut linux_usage =
            sampler::calculate_usage(&linux_before.primary, &linux_after.primary, host_cpu_count);
        for (name, before) in &linux_before.additional {
            if let Some((_, after)) = linux_after
                .additional
                .iter()
                .find(|(candidate, _)| candidate == name)
            {
                linux_usage.extend(sampler::calculate_usage(before, after, host_cpu_count));
            }
        }
        let mut windows_usage = Vec::new();
        if let (Some(before), Some(after)) = (&windows_before, &windows_after) {
            if before.host_logical_cpu_count != after.host_logical_cpu_count {
                warnings.push(format!(
                    "Windows logical CPU count changed ({} -> {})",
                    before.host_logical_cpu_count, after.host_logical_cpu_count
                ));
            }
            windows_usage =
                sampler::calculate_usage(&before.snapshot, &after.snapshot, host_cpu_count);
        }
        if collector_cpu_count != host_cpu_count {
            warnings.push(format!(
                "container collector results discarded because the Windows logical CPU count changed during sampling ({collector_cpu_count} -> {host_cpu_count})"
            ));
            wslc_result = None;
            docker_result = None;
        }
        let wslc_usage = match wslc_result {
            None => wslc::WslcUsage::default(),
            Some(result) => match result {
                Ok(result) => {
                    warnings.extend(result.warnings.iter().cloned());
                    result
                }
                Err(error) => {
                    warnings.push(format!("WSLC collector unavailable: {error}"));
                    wslc::WslcUsage::default()
                }
            },
        };
        let docker_usage = match docker_result {
            None => Vec::new(),
            Some(result) => match result {
                Ok(result) => {
                    warnings.extend(result.warnings);
                    result.resources
                }
                Err(error) => {
                    warnings.push(format!("Docker collector unavailable: {error}"));
                    Vec::new()
                }
            },
        };
        let hosts: Vec<_> = windows_usage
            .iter()
            .filter(|row| attribution::is_host_resource(row))
            .cloned()
            .collect();
        let applications = if should_collect_windows_metadata(&self.config, &windows_usage) {
            windows::application_metadata()
                .map(|metadata| windows_app::group_processes(&windows_usage, &metadata))
                .unwrap_or_else(|error| {
                    warnings.push(format!("Windows application metadata unavailable: {error}"));
                    windows_app::group_processes(&windows_usage, &Default::default())
                })
        } else {
            Vec::new()
        };
        let mut tree = attribution::build_tree_with_docker(
            host_cpu_count,
            &hosts,
            &linux_usage,
            &wslc_usage.resources,
            &docker_usage,
        );
        attribution::attach_wslc_processes(&mut tree, &wslc_usage.process_resources);
        tree.windows_applications = applications;
        let mut resources = linux_usage;
        resources.extend(windows_usage);
        resources.extend(wslc_usage.resources);
        if self.config.show_container_processes {
            resources.extend(
                wslc_usage
                    .process_resources
                    .iter()
                    .flat_map(|item| item.processes.iter().cloned()),
            );
            resources.extend(
                docker_usage
                    .iter()
                    .flat_map(|item| item.processes.iter().cloned()),
            );
        }
        resources.extend(docker_usage.into_iter().map(|item| item.resource));
        let mut snapshot = MonitorSnapshot::from_collected(resources, tree, warnings, &self.config);
        snapshot.host_cpu_percent = windows_after
            .and_then(|sample| sample.host_cpu)
            .zip(windows_before.and_then(|sample| sample.host_cpu))
            .and_then(|(after, before)| after.usage_since(before));
        Ok(snapshot)
    }
}

#[cfg(target_os = "windows")]
fn wsl_only_host_cpu_count() -> Result<u32, Box<dyn Error>> {
    windows::host_logical_cpu_count()
}

#[cfg(not(target_os = "windows"))]
fn wsl_only_host_cpu_count() -> Result<u32, Box<dyn Error>> {
    Ok(std::thread::available_parallelism().map_or(1, |count| count.get() as u32))
}

#[cfg(target_os = "windows")]
fn wsl_only_warning() -> &'static str {
    "--wsl-only uses the Windows logical CPU count but disables Windows host attribution"
}

#[cfg(not(target_os = "windows"))]
fn wsl_only_warning() -> &'static str {
    "--wsl-only uses the WSL-visible logical CPU count; exact host normalization and Windows host attribution require Windows interop"
}

fn should_collect_windows_metadata(
    config: &MonitorConfig,
    windows_usage: &[ResourceUsage],
) -> bool {
    config.collect_windows_applications
        && !config.wsl_only
        && windows_usage.iter().any(|row| {
            row.environment == crate::model::EnvironmentKind::Windows
                && row.kind == ResourceKind::Process
        })
}

fn apply_windows_application_view(
    resources: &mut Vec<ResourceUsage>,
    applications: &[WindowsApplicationUsage],
    config: &MonitorConfig,
) {
    if !config.collect_windows_applications || config.wsl_only {
        return;
    }
    resources.retain(|row| {
        row.environment != crate::model::EnvironmentKind::Windows
            || row.kind != ResourceKind::Process
    });
    resources.extend(
        applications
            .iter()
            .map(|application| application.resource.clone()),
    );
}

fn push_unique_warning(warnings: &mut Vec<String>, warning: String) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_windows_application_view, push_unique_warning, should_collect_windows_metadata,
        wsl_only_warning, MonitorConfig,
    };
    use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage, WindowsApplicationUsage};
    use std::time::Duration;

    fn prepare_flat_resources(rows: &mut Vec<ResourceUsage>, config: &MonitorConfig) {
        *rows = config.query().flat(rows);
    }

    fn config() -> MonitorConfig {
        MonitorConfig {
            sort: Default::default(),
            interval: Duration::from_secs(1),
            limit: 30,
            show_wsl_host: false,
            wsl_only: false,
            no_wslc: false,
            no_docker: false,
            hide_infra: false,
            show_container_processes: false,
            container_process_limit: 5,
            collect_windows_applications: true,
        }
    }

    fn resource(environment: EnvironmentKind, kind: ResourceKind, name: &str) -> ResourceUsage {
        ResourceUsage {
            environment,
            source: None,
            kind,
            id: name.to_string(),
            pid: Some(1),
            start_id: None,
            ppid: None,
            name: name.to_string(),
            args: None,
            cpu_percent: 1.0,
            cpu_time_seconds: None,
            memory_bytes: 0,
        }
    }

    #[test]
    fn wsl_only_skips_windows_metadata_collection() {
        let mut config = config();
        config.wsl_only = true;
        let windows = vec![resource(
            EnvironmentKind::Windows,
            ResourceKind::Process,
            "Teams",
        )];
        assert!(!should_collect_windows_metadata(&config, &windows));
    }

    #[test]
    fn wsl_only_warning_matches_the_native_platform() {
        #[cfg(target_os = "windows")]
        assert_eq!(
            wsl_only_warning(),
            "--wsl-only uses the Windows logical CPU count but disables Windows host attribution"
        );

        #[cfg(not(target_os = "windows"))]
        assert_eq!(
            wsl_only_warning(),
            "--wsl-only uses the WSL-visible logical CPU count; exact host normalization and Windows host attribution require Windows interop"
        );
    }

    #[test]
    fn pid_only_output_skips_windows_metadata_collection() {
        let mut config = config();
        config.collect_windows_applications = false;
        let windows = vec![resource(
            EnvironmentKind::Windows,
            ResourceKind::Process,
            "Teams",
        )];
        assert!(!should_collect_windows_metadata(&config, &windows));
    }

    #[test]
    fn pid_only_output_preserves_windows_process_rows() {
        let mut config = config();
        config.collect_windows_applications = false;
        let process = resource(EnvironmentKind::Windows, ResourceKind::Process, "chrome");
        let mut application = process.clone();
        application.kind = ResourceKind::Application;
        let applications = vec![WindowsApplicationUsage {
            resource: application,
            processes: vec![process.clone()],
        }];
        let mut resources = vec![process];

        apply_windows_application_view(&mut resources, &applications, &config);

        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].kind, ResourceKind::Process);
    }

    #[test]
    fn host_only_windows_rows_skip_metadata_collection() {
        let windows = vec![resource(
            EnvironmentKind::Windows,
            ResourceKind::Host,
            "vmmemwsl",
        )];
        assert!(!should_collect_windows_metadata(&config(), &windows));
    }

    #[test]
    fn flat_output_hides_hosts_but_preserves_resource_kinds() {
        let mut rows = vec![
            resource(EnvironmentKind::Windows, ResourceKind::Host, "vmmemwsl"),
            resource(EnvironmentKind::Wsl, ResourceKind::Infra, "plan9"),
            resource(
                EnvironmentKind::WslContainer,
                ResourceKind::Container,
                "container",
            ),
        ];
        prepare_flat_resources(&mut rows, &config());
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|row| row.kind == ResourceKind::Infra));
        assert!(rows.iter().any(|row| row.kind == ResourceKind::Container));
    }

    #[test]
    fn duplicate_collector_warnings_are_reported_once() {
        let mut warnings = Vec::new();
        push_unique_warning(&mut warnings, "same failure".into());
        push_unique_warning(&mut warnings, "same failure".into());
        assert_eq!(warnings, ["same failure"]);
    }

    #[test]
    fn flat_output_options_show_hosts_hide_infra_and_apply_limit() {
        let mut options = config();
        options.show_wsl_host = true;
        options.hide_infra = true;
        options.limit = 1;
        let mut rows = vec![
            resource(EnvironmentKind::Windows, ResourceKind::Host, "vmmemwsl"),
            resource(EnvironmentKind::Wsl, ResourceKind::Infra, "plan9"),
        ];
        prepare_flat_resources(&mut rows, &options);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].kind, ResourceKind::Host);
    }

    #[test]
    fn flat_limit_ranks_container_and_keeps_its_process_children_together() {
        let mut options = config();
        options.show_container_processes = true;
        options.limit = 1;
        let mut container = resource(
            EnvironmentKind::Docker,
            ResourceKind::Container,
            "container-id",
        );
        container.cpu_percent = 10.0;
        container.pid = None;
        let mut process = resource(EnvironmentKind::Docker, ResourceKind::Process, "simx");
        process.cpu_percent = 9.0;
        process.source = Some("container-id".to_string());
        let mut windows = resource(EnvironmentKind::Windows, ResourceKind::Process, "worker");
        windows.cpu_percent = 5.0;
        let mut rows = vec![process, windows, container];

        prepare_flat_resources(&mut rows, &options);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].kind, ResourceKind::Container);
        assert_eq!(rows[1].name, "simx");
    }
}
