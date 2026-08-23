use crate::attribution::{self, AttributionTree};
use crate::model::{ResourceKind, ResourceUsage};
use crate::{docker, linux, multiwsl, sampler, windows, wslc};
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
}

pub struct Monitor {
    config: MonitorConfig,
}

pub struct MonitorSnapshot {
    pub host_logical_cpu_count: u32,
    pub resources: Vec<ResourceUsage>,
    pub tree: AttributionTree,
    pub warnings: Vec<String>,
}

impl Monitor {
    pub fn new(config: MonitorConfig) -> Self {
        Self { config }
    }
    pub fn config_mut(&mut self) -> &mut MonitorConfig {
        &mut self.config
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
            Vec::new()
        } else {
            wslc::usage(host_cpu_count).unwrap_or_else(|error| {
                warnings.push(format!("WSLC collector unavailable: {error}"));
                Vec::new()
            })
        };
        let docker_usage = if self.config.no_docker {
            Vec::new()
        } else {
            docker::usage(host_cpu_count).unwrap_or_else(|error| {
                warnings.push(format!("Docker collector unavailable: {error}"));
                Vec::new()
            })
        };
        let hosts: Vec<_> = windows_usage
            .iter()
            .filter(|row| attribution::is_host_resource(row))
            .cloned()
            .collect();
        let mut tree = attribution::build_tree_with_docker(
            host_cpu_count,
            &hosts,
            &linux_usage,
            &wslc_usage,
            &docker_usage,
        );
        if self.config.hide_infra {
            attribution::hide_infra(&mut tree);
        }

        let mut resources = linux_usage;
        resources.extend(windows_usage);
        resources.extend(wslc_usage);
        resources.extend(docker_usage.into_iter().map(|item| item.resource));
        prepare_flat_resources(&mut resources, &self.config);
        Ok(MonitorSnapshot {
            host_logical_cpu_count: host_cpu_count,
            resources,
            tree,
            warnings,
        })
    }

    fn extra_wsl(&self, warnings: &mut Vec<String>) -> Vec<(String, crate::model::Snapshot)> {
        if self.config.wsl_only {
            return Vec::new();
        }
        multiwsl::snapshots().unwrap_or_else(|error| {
            warnings.push(format!(
                "additional WSL distro discovery unavailable: {error}"
            ));
            Vec::new()
        })
    }
}

fn prepare_flat_resources(resources: &mut Vec<ResourceUsage>, config: &MonitorConfig) {
    if !config.show_wsl_host {
        resources.retain(|row| !attribution::is_host_resource(row));
    }
    if config.hide_infra {
        resources.retain(|row| row.kind != ResourceKind::Infra);
    }
    resources.sort_by(|a, b| {
        b.cpu_percent
            .partial_cmp(&a.cpu_percent)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.memory_bytes.cmp(&a.memory_bytes))
    });
    resources.truncate(config.limit);
}

#[cfg(test)]
mod tests {
    use super::{prepare_flat_resources, MonitorConfig};
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
        }
    }

    fn resource(environment: EnvironmentKind, kind: ResourceKind, name: &str) -> ResourceUsage {
        ResourceUsage {
            environment,
            source: None,
            kind,
            id: name.to_string(),
            pid: Some(1),
            name: name.to_string(),
            cpu_percent: 1.0,
            memory_bytes: 0,
        }
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
}
