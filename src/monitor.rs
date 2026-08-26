use crate::attribution::{self, AttributionTree};
use crate::model::{ResourceKind, ResourceUsage};
use crate::{docker, linux, multiwsl, sampler, windows, windows_app, wslc};
use std::cmp::Ordering;
use std::error::Error;
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct MonitorConfig {
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
}

pub struct MonitorSnapshot {
    pub host_logical_cpu_count: u32,
    pub resources: Vec<ResourceUsage>,
    pub pid_resources: Vec<ResourceUsage>,
    pub tree: AttributionTree,
    pub warnings: Vec<String>,
}

impl Monitor {
    pub fn new(config: MonitorConfig) -> Self {
        Self { config }
    }
    pub fn sample(&mut self) -> Result<MonitorSnapshot, Box<dyn Error>> {
        let mut warnings = Vec::new();
        let linux_before = linux::snapshot()?;
        let extra_before = self.extra_wsl(&mut warnings);
        let windows_before = if self.config.wsl_only {
            None
        } else {
            Some(windows::snapshot()?)
        };
        thread::sleep(self.config.interval);
        let linux_after = linux::snapshot()?;
        let extra_after = self.extra_wsl(&mut warnings);
        let windows_after = if self.config.wsl_only {
            None
        } else {
            Some(windows::snapshot()?)
        };
        let host_cpu_count = windows_after.as_ref().map_or_else(
            || std::thread::available_parallelism().map_or(1, |count| count.get() as u32),
            |snapshot| snapshot.host_logical_cpu_count,
        );
        if self.config.wsl_only {
            warnings.push(
                "--wsl-only uses the WSL-visible logical CPU count; exact host normalization and Windows host attribution require Windows interop"
                    .to_string(),
            );
        }

        let mut linux_usage = sampler::calculate_usage(&linux_before, &linux_after, host_cpu_count);
        for (name, before) in &extra_before {
            if let Some((_, after)) = extra_after.iter().find(|(candidate, _)| candidate == name) {
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
        let wslc_usage = if self.config.wsl_only || self.config.no_wslc {
            wslc::WslcUsage::default()
        } else {
            match wslc::usage(host_cpu_count) {
                Ok(result) => {
                    warnings.extend(result.warnings.iter().cloned());
                    result
                }
                Err(error) => {
                    warnings.push(format!("WSLC collector unavailable: {error}"));
                    wslc::WslcUsage::default()
                }
            }
        };
        let docker_usage = if self.config.no_docker {
            Vec::new()
        } else {
            match docker::usage(host_cpu_count) {
                Ok(result) => {
                    warnings.extend(result.warnings);
                    result.resources
                }
                Err(error) => {
                    warnings.push(format!("Docker collector unavailable: {error}"));
                    Vec::new()
                }
            }
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
        if self.config.hide_infra {
            attribution::hide_infra(&mut tree);
        }

        let mut resources = linux_usage;
        resources.extend(windows_usage);
        resources.extend(wslc_usage.resources);
        if self.config.show_container_processes {
            resources.extend(wslc_usage.process_resources.iter().flat_map(|item| {
                let mut processes = item.processes.clone();
                processes.sort_by(|a, b| {
                    b.cpu_percent
                        .partial_cmp(&a.cpu_percent)
                        .unwrap_or(Ordering::Equal)
                });
                processes.truncate(self.config.container_process_limit);
                processes
            }));
            resources.extend(docker_usage.iter().flat_map(|item| {
                let mut processes = item.processes.clone();
                processes.sort_by(|a, b| {
                    b.cpu_percent
                        .partial_cmp(&a.cpu_percent)
                        .unwrap_or(Ordering::Equal)
                });
                processes.truncate(self.config.container_process_limit);
                processes
            }));
        }
        resources.extend(docker_usage.into_iter().map(|item| item.resource));
        let mut pid_resources = resources.clone();
        prepare_flat_resources(&mut pid_resources, &self.config);
        resources.retain(|row| {
            row.environment != crate::model::EnvironmentKind::Windows
                || row.kind != ResourceKind::Process
        });
        resources.extend(
            tree.windows_applications
                .iter()
                .map(|application| application.resource.clone()),
        );
        prepare_flat_resources(&mut resources, &self.config);
        Ok(MonitorSnapshot {
            host_logical_cpu_count: host_cpu_count,
            resources,
            pid_resources,
            tree,
            warnings,
        })
    }

    fn extra_wsl(&self, warnings: &mut Vec<String>) -> Vec<(String, crate::model::Snapshot)> {
        if self.config.wsl_only {
            return Vec::new();
        }
        match multiwsl::snapshots() {
            Ok(batch) => {
                for (source, error) in &batch.failures {
                    push_unique_warning(
                        warnings,
                        format!("additional WSL {source} unavailable: {error}"),
                    );
                }
                batch.snapshots
            }
            Err(error) => {
                push_unique_warning(
                    warnings,
                    format!("additional WSL distro discovery unavailable: {error}"),
                );
                Vec::new()
            }
        }
    }
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

fn push_unique_warning(warnings: &mut Vec<String>, warning: String) {
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

pub(crate) fn prepare_flat_resources(resources: &mut Vec<ResourceUsage>, config: &MonitorConfig) {
    if !config.show_wsl_host {
        resources.retain(|row| !attribution::is_host_resource(row));
    }
    if config.hide_infra {
        resources.retain(|row| row.kind != ResourceKind::Infra);
    }
    let mut container_processes = Vec::new();
    let mut top_level = Vec::new();
    for row in std::mem::take(resources) {
        if matches!(
            row.environment,
            crate::model::EnvironmentKind::Docker | crate::model::EnvironmentKind::WslContainer
        ) && row.kind == ResourceKind::Process
            && row.source.is_some()
        {
            container_processes.push(row);
        } else {
            top_level.push(row);
        }
    }
    top_level.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
    });
    top_level.truncate(config.limit);

    container_processes.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(Ordering::Equal)
    });
    for row in top_level {
        let container_id = (matches!(
            row.environment,
            crate::model::EnvironmentKind::Docker | crate::model::EnvironmentKind::WslContainer
        ) && row.kind == ResourceKind::Container)
            .then(|| row.id.clone());
        resources.push(row);
        if let Some(container_id) = container_id {
            resources.extend(
                container_processes
                    .iter()
                    .filter(|process| process.source.as_deref() == Some(container_id.as_str()))
                    .cloned(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        prepare_flat_resources, push_unique_warning, should_collect_windows_metadata, MonitorConfig,
    };
    use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};
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
