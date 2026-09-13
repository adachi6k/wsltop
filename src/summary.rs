//! Independent observations, deliberately not an additive partition of host usage.
use crate::attribution;
use crate::model::{EnvironmentKind, ResourceKind, ResourceUsage};

pub const ENVIRONMENTS: [EnvironmentKind; 4] = [
    EnvironmentKind::Windows,
    EnvironmentKind::Wsl,
    EnvironmentKind::WslContainer,
    EnvironmentKind::Docker,
];

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub cpu_percent: f64,
    pub memory_bytes: u64,
}

/// None means unavailable/disabled/warming up; Some(zero) is a successful empty sample.
#[derive(Debug, Clone, Default)]
pub struct EnvironmentSummary(pub [Option<Usage>; 4]);

impl EnvironmentSummary {
    /// CPU covers the shared WSL kernel; RAM keeps the observed process RSS sum.
    /// Never substitute a partial process sum when the system counter is absent.
    pub fn set_wsl_cpu(&mut self, cpu: Option<f64>) {
        self.0[1] = self.0[1].and_then(|mut usage| {
            usage.cpu_percent = cpu?;
            Some(usage)
        });
    }
    pub fn collect(rows: &[ResourceUsage], available: [bool; 4]) -> Self {
        Self(std::array::from_fn(|index| {
            available[index].then(|| {
                rows.iter()
                    .filter(|row| row.environment == ENVIRONMENTS[index])
                    .filter(|row| match row.environment {
                        EnvironmentKind::Windows => {
                            matches!(row.kind, ResourceKind::Process | ResourceKind::Infra)
                                && !attribution::is_host_resource(row)
                        }
                        EnvironmentKind::Wsl => {
                            matches!(row.kind, ResourceKind::Process | ResourceKind::Infra)
                        }
                        EnvironmentKind::WslContainer | EnvironmentKind::Docker => {
                            row.kind == ResourceKind::Container
                        }
                    })
                    .fold(Usage::default(), |mut sum, row| {
                        sum.cpu_percent += row.cpu_percent;
                        sum.memory_bytes = sum.memory_bytes.saturating_add(row.memory_bytes);
                        sum
                    })
            })
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_cpu_overrides_only_wsl_cpu_preserving_memory_and_container_totals() {
        let mut summary = EnvironmentSummary(
            [Some(Usage {
                cpu_percent: 10.0,
                memory_bytes: 4096,
            }); 4],
        );
        summary.set_wsl_cpu(Some(85.0));
        assert_eq!(
            summary.0[1].unwrap(),
            Usage {
                cpu_percent: 85.0,
                memory_bytes: 4096
            }
        );
        for index in [0, 2, 3] {
            assert_eq!(summary.0[index].unwrap().cpu_percent, 10.0);
        }
        summary.set_wsl_cpu(None);
        assert!(summary.0[1].is_none());
    }

    #[test]
    fn excludes_vm_hosts_applications_and_container_children_but_keeps_inclusive_wsl() {
        let mut win = crate::query::tests::row("editor", 2.0, 100);
        win.environment = EnvironmentKind::Windows;
        let mut vm = win.clone();
        vm.name = "vmmemWSL".into();
        vm.kind = ResourceKind::Host;
        vm.cpu_percent = 40.0;
        let mut app = win.clone();
        app.kind = ResourceKind::Application;
        let mut wsl = win.clone();
        wsl.environment = EnvironmentKind::Wsl;
        wsl.cpu_percent = 8.0;
        let mut container = wsl.clone();
        container.environment = EnvironmentKind::Docker;
        container.kind = ResourceKind::Container;
        container.cpu_percent = 3.0;
        let mut child = container.clone();
        child.kind = ResourceKind::Process;
        let result = EnvironmentSummary::collect(&[win, vm, app, wsl, container, child], [true; 4]);
        assert_eq!(result.0[0].unwrap().cpu_percent, 2.0);
        assert_eq!(result.0[1].unwrap().cpu_percent, 8.0);
        assert_eq!(result.0[2], Some(Usage::default()));
        assert_eq!(
            result.0[3].unwrap(),
            Usage {
                cpu_percent: 3.0,
                memory_bytes: 100
            }
        );
        assert_eq!(EnvironmentSummary::collect(&[], [false; 4]).0, [None; 4]);
    }
}
